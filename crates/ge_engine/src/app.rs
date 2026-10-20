//! The application: one ECS world, its tick-group schedules and the plugins that fill them
//! (lore §3.3, §6.3, §6.5). Games never write a main loop; they build an `App` and call `run`.

use std::time::Duration;

use ge_ecs::prelude::*;
use ge_ecs::schedule::{IntoScheduleConfigs, Schedule, ScheduleSystem};
use ge_platform::WindowDesc;
use tracing::{debug, info, info_span, warn};

use crate::input::PlatformEvents;
use crate::plugin::{Plugin, PluginError, Plugins, build_order};
use crate::{EngineError, TickGroup, runner};

ge_core::declare_cvar!(pub static P_FIXED_HZ: f32 = (
    "p.FixedHz",
    60.0,
    "Fixed simulation rate in steps per second (the FixedUpdate tick group)"
));

/// Cooperative exit (UE's `RequestEngineExit`): any system may request it; the loop finishes the
/// current frame and shuts down. The first reason wins.
#[derive(Resource, Clone, Debug, Default)]
pub struct ExitRequest {
    reason: Option<String>,
}

impl ExitRequest {
    pub fn request(&mut self, reason: impl Into<String>) {
        if self.reason.is_none() {
            let reason = reason.into();
            info!(reason, "exit requested");
            self.reason = Some(reason);
        }
    }

    pub fn reason(&self) -> Option<&str> {
        self.reason.as_deref()
    }
}

pub struct App {
    world: World,
    plugins: Vec<Box<dyn Plugin>>,
    built: bool,
    pub(crate) window: WindowDesc,
    args: Vec<String>,
}

impl std::fmt::Debug for App {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("App")
            .field(
                "plugins",
                &self.plugins.iter().map(|p| p.name()).collect::<Vec<_>>(),
            )
            .field("built", &self.built)
            .finish_non_exhaustive()
    }
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

impl App {
    /// An empty app: every tick-group schedule exists, plus the core resources (`Time`,
    /// `FixedTime`, `ExitRequest`, `PlatformEvents`). Features come from plugins.
    pub fn new() -> Self {
        let mut world = World::new();
        for group in TickGroup::ALL {
            world.add_schedule(Schedule::new(group));
        }
        world.init_resource::<Time>();
        world.insert_resource(FixedTime::from_hz(f64::from(P_FIXED_HZ.get())));
        world.init_resource::<ExitRequest>();
        world.init_resource::<PlatformEvents>();
        Self {
            world,
            plugins: Vec::new(),
            built: false,
            window: WindowDesc::default(),
            args: std::env::args().collect(),
        }
    }

    pub fn set_window(&mut self, window: WindowDesc) -> &mut Self {
        self.window = window;
        self
    }

    pub fn add_plugins(&mut self, plugins: impl Plugins) -> &mut Self {
        plugins.add_to(self);
        self
    }

    pub(crate) fn push_plugin(&mut self, plugin: Box<dyn Plugin>) {
        self.plugins.push(plugin);
    }

    pub fn add_systems<M>(
        &mut self,
        group: TickGroup,
        systems: impl IntoScheduleConfigs<ScheduleSystem, M>,
    ) -> &mut Self {
        self.world
            .resource_mut::<ge_ecs::schedule::Schedules>()
            .add_systems(group, systems);
        self
    }

    pub fn insert_resource<R: Resource>(&mut self, resource: R) -> &mut Self {
        self.world.insert_resource(resource);
        self
    }

    pub fn init_resource<R: Resource + Default>(&mut self) -> &mut Self {
        self.world.init_resource::<R>();
        self
    }

    pub fn world(&self) -> &World {
        &self.world
    }

    pub fn world_mut(&mut self) -> &mut World {
        &mut self.world
    }

    /// Builds every added plugin in dependency order. Called by [`App::run`]; tests call it
    /// directly to drive an app without a window.
    pub fn finish(&mut self) -> Result<(), EngineError> {
        if self.built {
            return Ok(());
        }
        let _span = info_span!("App::BuildPlugins").entered();
        let plugins = std::mem::take(&mut self.plugins);
        let meta: Vec<_> = plugins
            .iter()
            .map(|p| (p.name(), p.dependencies()))
            .collect();
        let order = build_order(&meta)?;
        for i in order {
            let plugin = &plugins[i];
            debug!(plugin = plugin.name(), "building plugin");
            plugin.build(self);
            if !self.plugins.is_empty() {
                return Err(PluginError::AddedDuringBuild(plugin.name()).into());
            }
        }
        info!(
            plugins = ?plugins.iter().map(|p| p.name()).collect::<Vec<_>>(),
            "plugins built"
        );
        self.plugins = plugins;
        self.built = true;
        Ok(())
    }

    /// Runs the `Startup` tick group once.
    pub fn startup(&mut self) {
        let _span = info_span!("App::Startup").entered();
        self.world.run_schedule(TickGroup::Startup);
    }

    /// One frame of simulation: every tick group in [`TickGroup::FRAME`] order, with
    /// `FixedUpdate` run as many times as the accumulator owes.
    pub fn update(&mut self, raw_delta: Duration) {
        let dt = self.world.resource_mut::<Time>().advance(raw_delta);

        for group in [TickGroup::First, TickGroup::PrePhysics] {
            self.run_group(group);
        }

        {
            let mut fixed = self.world.resource_mut::<FixedTime>();
            fixed.set_hz(f64::from(P_FIXED_HZ.get()));
            let dropped = fixed.accumulate(dt);
            if !dropped.is_zero() {
                debug!(?dropped, "simulation fell behind; dropping time");
            }
        }
        while self.world.resource_mut::<FixedTime>().expend() {
            self.run_group(TickGroup::FixedUpdate);
        }

        for group in [
            TickGroup::PostPhysics,
            TickGroup::Animation,
            TickGroup::PostUpdate,
            TickGroup::Extract,
        ] {
            self.run_group(group);
        }
        self.world.resource_mut::<PlatformEvents>().0.clear();
    }

    fn run_group(&mut self, group: TickGroup) {
        let _span = info_span!("TickGroup", ?group).entered();
        self.world.run_schedule(group);
    }

    pub fn exit_requested(&self) -> Option<&str> {
        self.world.resource::<ExitRequest>().reason()
    }

    pub(crate) fn request_exit(&mut self, reason: &str) {
        self.world.resource_mut::<ExitRequest>().request(reason);
    }

    /// Startup phases in order (CLAUDE.md §6): logging → command-line cvars → plugins → window,
    /// RHI and global shaders → `Startup` → frame loop → shutdown.
    pub fn run(&mut self) -> Result<(), EngineError> {
        ge_core::log::init_logging();
        info!(version = env!("CARGO_PKG_VERSION"), "engine starting");
        for e in ge_core::cvar::apply_command_line(&self.args) {
            warn!(error = %e, "ignoring command-line cvar");
        }
        self.finish()?;
        let mut engine = runner::Engine::new(self);
        ge_platform::run(&mut engine)?;
        info!("engine stopped");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Resource, Default)]
    struct Log(Vec<&'static str>);

    fn log(name: &'static str) -> impl FnMut(ResMut<Log>) {
        move |mut l: ResMut<Log>| l.0.push(name)
    }

    const FRAME_60HZ: Duration = Duration::from_nanos(16_666_667);

    #[test]
    fn tick_groups_run_in_claude_md_order() {
        let mut app = App::new();
        app.init_resource::<Log>()
            .add_systems(TickGroup::Startup, log("Startup"))
            .add_systems(TickGroup::Extract, log("Extract"))
            .add_systems(TickGroup::PostUpdate, log("PostUpdate"))
            .add_systems(TickGroup::Animation, log("Animation"))
            .add_systems(TickGroup::PostPhysics, log("PostPhysics"))
            .add_systems(TickGroup::FixedUpdate, log("FixedUpdate"))
            .add_systems(TickGroup::PrePhysics, log("PrePhysics"))
            .add_systems(TickGroup::First, log("First"));
        app.finish().unwrap();
        app.startup();
        app.update(FRAME_60HZ);
        assert_eq!(
            app.world().resource::<Log>().0,
            [
                "Startup",
                "First",
                "PrePhysics",
                "FixedUpdate",
                "PostPhysics",
                "Animation",
                "PostUpdate",
                "Extract"
            ]
        );
    }

    /// Simulated distance after 1 s of frames, moving 1 unit/s in `FixedUpdate`.
    fn distance_after_one_second(frame: Duration) -> f32 {
        #[derive(Resource, Default)]
        struct Position(f32);
        let mut app = App::new();
        app.init_resource::<Position>().add_systems(
            TickGroup::FixedUpdate,
            |mut p: ResMut<Position>, f: Res<FixedTime>| {
                p.0 += f.step_secs();
            },
        );
        app.finish().unwrap();
        let frames = (1.0 / frame.as_secs_f64()).round() as u32;
        for _ in 0..frames {
            app.update(frame);
        }
        app.world().resource::<Position>().0
    }

    #[test]
    fn movement_is_frame_rate_independent() {
        let step = 1.0 / 60.0;
        for fps in [30.0, 60.0, 144.0, 240.0] {
            let d = distance_after_one_second(Duration::from_secs_f64(1.0 / fps));
            assert!((d - 1.0).abs() <= step + 1e-4, "{fps} fps moved {d}");
        }
    }

    #[test]
    fn exit_request_keeps_first_reason() {
        let mut app = App::new();
        assert_eq!(app.exit_requested(), None);
        app.request_exit("first");
        app.request_exit("second");
        assert_eq!(app.exit_requested(), Some("first"));
    }

    struct Named(&'static str, &'static [&'static str]);
    impl Plugin for Named {
        fn name(&self) -> &'static str {
            self.0
        }
        fn dependencies(&self) -> &'static [&'static str] {
            self.1
        }
        fn build(&self, app: &mut App) {
            app.world_mut().resource_mut::<Log>().0.push(self.0);
        }
    }

    #[test]
    fn plugins_build_in_dependency_order() {
        let mut app = App::new();
        app.init_resource::<Log>()
            .add_plugins((Named("game", &["render"]), Named("render", &[])));
        app.finish().unwrap();
        assert_eq!(app.world().resource::<Log>().0, ["render", "game"]);
    }

    #[test]
    fn missing_plugin_dependency_fails_finish() {
        let mut app = App::new();
        app.init_resource::<Log>()
            .add_plugins(Named("game", &["physics"]));
        let err = app.finish().unwrap_err().to_string();
        assert!(err.contains("physics"), "{err}");
    }
}

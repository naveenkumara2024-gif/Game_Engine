//! Model instances (plan 0005, decision 11): a [`SceneRoot`] holds a strong handle to a glTF
//! model; once the model is loaded, [`spawn_scenes`] spawns its node tree as children (with
//! `Name`, `Transform` and `MeshRenderer`s). When the model is hot-reloaded the children are
//! re-spawned — per-instance edits to them are lost until reflection-based diffs (M6).

use ge_asset::{AssetServer, Handle, ModelAsset};
use ge_ecs::prelude::*;
use tracing::info;

use crate::{MeshKind, MeshRenderer};

/// An instance of a model asset at this entity's transform.
#[derive(Component, Clone, Debug)]
#[require(Transform)]
pub struct SceneRoot(pub Handle<ModelAsset>);

/// Which model version this root's children were spawned from.
#[derive(Component, Copy, Clone, Debug, PartialEq, Eq)]
pub struct SceneSpawned {
    pub version: u32,
}

/// Marks entities spawned from a model (replaced on reload).
#[derive(Component, Copy, Clone, Debug, Default)]
pub struct SceneNode;

pub fn spawn_scenes(
    mut commands: Commands,
    server: Option<Res<AssetServer>>,
    roots: Query<(Entity, &SceneRoot, Option<&SceneSpawned>, Option<&Children>)>,
    nodes: Query<(), With<SceneNode>>,
) {
    let Some(server) = server else { return };
    for (root, scene, spawned, children) in &roots {
        let id = scene.0.id();
        let version = server.version(id);
        let Some(model) = server.get(id) else {
            continue;
        };
        if spawned.is_some_and(|s| s.version == version) {
            continue;
        }
        // Replace children from an older version of the model.
        if let Some(children) = children {
            for &child in children.iter() {
                if nodes.get(child).is_ok() {
                    commands.entity(child).despawn();
                }
            }
        }
        let mut stack: Vec<(usize, Entity)> = model.roots.iter().map(|&n| (n, root)).collect();
        while let Some((index, parent)) = stack.pop() {
            let node = &model.nodes[index];
            let transform = Transform {
                translation: Vec3::from(node.translation),
                rotation: Quat::from_array(node.rotation).normalize(),
                scale: Vec3::from(node.scale),
            };
            let mut entity = commands.spawn((
                Name::new(node.name.clone()),
                transform,
                SceneNode,
                ChildOf(parent),
            ));
            let entity_id = entity.id();
            // One primitive renders on the node itself; several get a child each.
            let prims: Vec<_> = node
                .primitives
                .iter()
                .filter_map(|p| {
                    p.mesh
                        .id
                        .map(|m| (m, p.material.as_ref().and_then(|m| m.id)))
                })
                .collect();
            match prims.as_slice() {
                [] => {}
                [(mesh, material)] => {
                    entity.insert(MeshRenderer {
                        mesh: MeshKind::Asset(*mesh),
                        tint: Vec4::ONE,
                        material: *material,
                    });
                }
                many => {
                    for (i, (mesh, material)) in many.iter().enumerate() {
                        commands.spawn((
                            Name::new(format!("{}.{i}", node.name)),
                            Transform::IDENTITY,
                            SceneNode,
                            ChildOf(entity_id),
                            MeshRenderer {
                                mesh: MeshKind::Asset(*mesh),
                                tint: Vec4::ONE,
                                material: *material,
                            },
                        ));
                    }
                }
            }
            stack.extend(node.children.iter().map(|&c| (c, entity_id)));
        }
        commands.entity(root).insert(SceneSpawned { version });
        info!(model = %scene.0.source(), version, nodes = model.nodes.len(), "model instance spawned");
    }
}

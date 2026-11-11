use std::collections::HashMap;
use std::path::Path;

use super::{DEFAULT_COLOR, Material, ModelError, Primitive, Scene, UpAxis, Vertex};

const LOAD_OPTIONS: tobj::LoadOptions = tobj::LoadOptions {
    triangulate: true,
    single_index: true,
    ignore_points: true,
    ignore_lines: true,
};

fn load_texture(dir: &Path, name: &str) -> Option<image::RgbaImage> {
    let path = dir.join(name.replace('\\', "/"));
    match image::open(&path) {
        Ok(img) => Some(img.to_rgba8()),
        Err(e) => {
            log::warn!("cannot load texture {}: {e}", path.display());
            None
        }
    }
}

pub(super) fn load_obj(path: &Path) -> Result<Scene, ModelError> {
    let (models, mtl) =
        tobj::load_obj(path, &LOAD_OPTIONS).map_err(|e| ModelError::Parse(e.to_string()))?;
    let mtl = mtl.unwrap_or_else(|e| {
        log::warn!(
            "cannot read the material library of {}: {e}",
            path.display()
        );
        Vec::new()
    });
    let dir = path.parent().unwrap_or(Path::new("."));

    let mut textures = Vec::new();
    let mut texture_by_name: HashMap<String, Option<usize>> = HashMap::new();
    let mut materials: Vec<Material> = mtl
        .iter()
        .map(|m| {
            let [r, g, b] =
                m.diffuse
                    .unwrap_or([DEFAULT_COLOR[0], DEFAULT_COLOR[1], DEFAULT_COLOR[2]]);
            let texture = m.diffuse_texture.as_ref().and_then(|name| {
                *texture_by_name.entry(name.clone()).or_insert_with(|| {
                    let img = load_texture(dir, name)?;
                    textures.push(img);
                    Some(textures.len() - 1)
                })
            });
            Material {
                base_color: [r, g, b, 1.0],
                texture,
            }
        })
        .collect();
    let mut default_material = None;

    let mut vertices = Vec::new();
    let mut indices = Vec::new();
    let mut primitives = Vec::new();
    for model in &models {
        let mesh = &model.mesh;
        let count = mesh.positions.len() / 3;
        if mesh.indices.iter().any(|&i| i as usize >= count) {
            return Err(ModelError::Parse("OBJ face index out of range".into()));
        }
        let has_normals = mesh.normals.len() >= count * 3;
        let has_uvs = mesh.texcoords.len() >= count * 2;
        let base = u32::try_from(vertices.len())
            .map_err(|_| ModelError::Parse("too many vertices".into()))?;
        for i in 0..count {
            let p = &mesh.positions[3 * i..3 * i + 3];
            vertices.push(Vertex {
                position: [p[0], p[1], p[2]],
                normal: if has_normals {
                    let n = &mesh.normals[3 * i..3 * i + 3];
                    [n[0], n[1], n[2]]
                } else {
                    [0.0; 3]
                },
                // OBJ's v axis points up, glTF's points down.
                uv: if has_uvs {
                    [mesh.texcoords[2 * i], 1.0 - mesh.texcoords[2 * i + 1]]
                } else {
                    [0.0; 2]
                },
            });
        }
        let start = u32::try_from(indices.len())
            .map_err(|_| ModelError::Parse("too many indices".into()))?;
        indices.extend(mesh.indices.iter().map(|i| i + base));
        let material = match mesh.material_id.filter(|&m| m < materials.len()) {
            Some(m) => m,
            None => *default_material.get_or_insert_with(|| {
                materials.push(Material {
                    base_color: DEFAULT_COLOR,
                    texture: None,
                });
                materials.len() - 1
            }),
        };
        primitives.push(Primitive {
            indices: start..start + mesh.indices.len() as u32,
            material,
        });
    }

    let mut scene = Scene {
        vertices,
        indices,
        primitives,
        materials,
        textures,
        camera: None,
        up: UpAxis::Y,
    };
    scene.fill_missing_normals();
    Ok(scene)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::fixture;

    fn obj_dir_with_mtl(mtl: &str) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::copy(fixture("cube.obj"), dir.path().join("cube.obj")).unwrap();
        std::fs::write(dir.path().join("cube.mtl"), mtl).unwrap();
        dir
    }

    #[test]
    fn obj_cube_reads_mtl_color() {
        let scene = load_obj(&fixture("cube.obj")).unwrap();
        assert_eq!(scene.triangle_count(), 12);
        assert_eq!(scene.up, UpAxis::Y);
        let material = &scene.materials[scene.primitives[0].material];
        assert_eq!(material.base_color, [1.0, 0.0, 0.0, 1.0]);
    }

    #[test]
    fn obj_without_mtl_uses_default_color() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::copy(fixture("cube.obj"), dir.path().join("cube.obj")).unwrap();
        let scene = load_obj(&dir.path().join("cube.obj")).unwrap();
        assert_eq!(scene.triangle_count(), 12);
        assert!(
            scene
                .materials
                .iter()
                .all(|m| m.base_color == DEFAULT_COLOR)
        );
    }

    #[test]
    fn obj_with_missing_texture_still_loads() {
        let dir = obj_dir_with_mtl("newmtl Mat\nKd 1 0 0\nmap_Kd missing.png\n");
        let scene = load_obj(&dir.path().join("cube.obj")).unwrap();
        assert!(scene.textures.is_empty());
        assert!(scene.materials.iter().all(|m| m.texture.is_none()));
    }

    #[test]
    fn obj_texture_is_loaded() {
        let dir = obj_dir_with_mtl("newmtl Mat\nKd 1 1 1\nmap_Kd green.png\n");
        image::RgbaImage::from_pixel(2, 2, image::Rgba([0, 255, 0, 255]))
            .save(dir.path().join("green.png"))
            .unwrap();
        let scene = load_obj(&dir.path().join("cube.obj")).unwrap();
        let tex = scene.materials[scene.primitives[0].material]
            .texture
            .unwrap();
        assert_eq!(scene.textures[tex].get_pixel(0, 0).0, [0, 255, 0, 255]);
    }

    #[test]
    fn malformed_obj_is_err_not_panic() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad.obj");
        std::fs::write(&path, "v 0 0 0\nf 1 2 9\n").unwrap();
        assert!(load_obj(&path).is_err());
    }
}

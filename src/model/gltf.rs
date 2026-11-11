use std::path::Path;

use glam::{Mat3, Mat4, Vec3};

use super::{DEFAULT_COLOR, Material, ModelError, Primitive, Scene, SceneCamera, UpAxis, Vertex};

impl From<::gltf::Error> for ModelError {
    fn from(e: ::gltf::Error) -> Self {
        match e {
            ::gltf::Error::Io(e) => ModelError::Io(e),
            e => ModelError::Parse(e.to_string()),
        }
    }
}

pub(super) fn load_gltf(path: &Path) -> Result<Scene, ModelError> {
    let (doc, buffers, images) = ::gltf::import(path)?;

    let mut textures = Vec::new();
    let image_to_texture: Vec<Option<usize>> = images
        .iter()
        .map(|img| {
            to_rgba(img).map(|t| {
                textures.push(t);
                textures.len() - 1
            })
        })
        .collect();

    let mut materials: Vec<Material> = doc
        .materials()
        .map(|m| {
            let pbr = m.pbr_metallic_roughness();
            let texture = pbr
                .base_color_texture()
                .and_then(|t| image_to_texture[t.texture().source().index()]);
            Material { base_color: pbr.base_color_factor(), texture }
        })
        .collect();

    let mut scene = Scene {
        vertices: Vec::new(),
        indices: Vec::new(),
        primitives: Vec::new(),
        materials: Vec::new(),
        textures,
        camera: None,
        up: UpAxis::Y,
    };
    let mut default_material = None;

    let gltf_scene = doc.default_scene().or_else(|| doc.scenes().next());
    let mut stack: Vec<(::gltf::Node, Mat4)> = gltf_scene
        .into_iter()
        .flat_map(|s| s.nodes().collect::<Vec<_>>())
        .rev()
        .map(|n| (n, Mat4::IDENTITY))
        .collect();
    // Popping from a stack visits nodes depth-first in document order.
    while let Some((node, parent)) = stack.pop() {
        let world = parent * Mat4::from_cols_array_2d(&node.transform().matrix());
        if scene.camera.is_none() {
            scene.camera = node.camera().and_then(|c| camera_at(&c, &world));
        }
        if let Some(mesh) = node.mesh() {
            for prim in mesh.primitives() {
                if prim.mode() != ::gltf::mesh::Mode::Triangles {
                    continue;
                }
                let material = match prim.material().index() {
                    Some(i) => i,
                    None => *default_material.get_or_insert_with(|| {
                        materials.push(Material { base_color: DEFAULT_COLOR, texture: None });
                        materials.len() - 1
                    }),
                };
                append_primitive(&mut scene, &prim, &buffers, &world, material)?;
            }
        }
        let children: Vec<_> = node.children().collect();
        stack.extend(children.into_iter().rev().map(|c| (c, world)));
    }

    scene.materials = materials;
    scene.fill_missing_normals();
    Ok(scene)
}

fn camera_at(camera: &::gltf::Camera, world: &Mat4) -> Option<SceneCamera> {
    let ::gltf::camera::Projection::Perspective(p) = camera.projection() else {
        return None;
    };
    let eye = world.transform_point3(Vec3::ZERO);
    let forward = world.transform_vector3(Vec3::NEG_Z).normalize_or_zero();
    let up = world.transform_vector3(Vec3::Y).normalize_or_zero();
    let finite = eye.is_finite() && forward.is_finite() && up.is_finite();
    if !finite || forward == Vec3::ZERO || up == Vec3::ZERO {
        return None;
    }
    let aspect = p.aspect_ratio().filter(|a| a.is_finite() && *a > 0.0);
    Some(SceneCamera { eye, target: eye + forward, up, yfov: p.yfov(), aspect })
}

fn append_primitive(
    scene: &mut Scene,
    prim: &::gltf::Primitive,
    buffers: &[::gltf::buffer::Data],
    world: &Mat4,
    material: usize,
) -> Result<(), ModelError> {
    let reader = prim.reader(|b| buffers.get(b.index()).map(|d| &d.0[..]));
    let Some(positions) = reader.read_positions() else { return Ok(()) };
    let positions: Vec<[f32; 3]> = positions.collect();
    let mut normals = reader.read_normals().map(|n| n.collect::<Vec<_>>()).unwrap_or_default();
    let mut uvs = reader
        .read_tex_coords(0)
        .map(|t| t.into_f32().collect::<Vec<_>>())
        .unwrap_or_default();
    normals.resize(positions.len(), [0.0; 3]);
    uvs.resize(positions.len(), [0.0; 2]);

    let indices: Vec<u32> = match reader.read_indices() {
        Some(i) => i.into_u32().collect(),
        None => (0..positions.len() as u32).collect(),
    };
    if indices.iter().any(|&i| i as usize >= positions.len()) {
        return Err(ModelError::Parse("index out of range".into()));
    }

    let normal_matrix = Mat3::from_mat4(*world).inverse().transpose();
    let base = scene.vertices.len() as u32;
    scene.vertices.extend(positions.iter().zip(&normals).zip(&uvs).map(|((p, n), uv)| {
        Vertex {
            position: world.transform_point3(Vec3::from(*p)).to_array(),
            normal: (normal_matrix * Vec3::from(*n)).normalize_or_zero().to_array(),
            uv: *uv,
        }
    }));

    let start = scene.indices.len() as u32;
    let mirrored = world.determinant() < 0.0;
    for tri in indices.chunks_exact(3) {
        let (b, c) = if mirrored { (tri[2], tri[1]) } else { (tri[1], tri[2]) };
        scene.indices.extend([base + tri[0], base + b, base + c]);
    }
    scene.primitives.push(Primitive { indices: start..scene.indices.len() as u32, material });
    Ok(())
}

fn to_rgba(img: &::gltf::image::Data) -> Option<image::RgbaImage> {
    use ::gltf::image::Format;
    let n = (img.width * img.height) as usize;
    let mut out = Vec::with_capacity(n * 4);
    match img.format {
        Format::R8 => img.pixels.iter().for_each(|&r| out.extend([r, r, r, 255])),
        Format::R8G8 => img.pixels.chunks_exact(2).for_each(|p| out.extend([p[0], p[1], 0, 255])),
        Format::R8G8B8 => img.pixels.chunks_exact(3).for_each(|p| out.extend([p[0], p[1], p[2], 255])),
        Format::R8G8B8A8 => out.extend_from_slice(&img.pixels),
        _ => return None,
    }
    image::RgbaImage::from_raw(img.width, img.height, out)
}

#[cfg(test)]
mod tests {
    use super::super::{ModelError, UpAxis, fixture, load};
    use glam::Vec3;

    #[test]
    fn glb_cube_has_12_triangles_and_red_material() {
        let s = load(&fixture("cube.glb")).unwrap();
        assert_eq!(s.triangle_count(), 12);
        let c = s.materials[s.primitives[0].material].base_color;
        assert!((c[0] - 1.0).abs() < 1e-3 && c[1] < 1e-3 && c[2] < 1e-3, "{c:?}");
        assert_eq!(s.up, UpAxis::Y);
        let b = s.bounds().unwrap();
        assert!((b.max - b.min - Vec3::splat(2.0)).length() < 1e-3);
    }

    #[test]
    fn glb_camera_is_read() {
        // Blender (4,-4,3) Z-up becomes glTF (4,3,4) Y-up.
        let cam = load(&fixture("cube.glb")).unwrap().camera.unwrap();
        assert!((cam.eye - Vec3::new(4.0, 3.0, 4.0)).length() < 1e-3, "{:?}", cam.eye);
        assert!((cam.target - cam.eye).normalize().dot(-cam.eye.normalize()) > 0.999);
        let aspect = cam.aspect.expect("cube.glb camera has an aspect ratio");
        assert!((aspect - 16.0 / 9.0).abs() < 1e-3, "{aspect}");
    }

    #[test]
    fn gltf_separate_loads_like_glb() {
        assert_eq!(load(&fixture("cube.gltf")).unwrap().triangle_count(), 12);
    }

    #[test]
    fn glb_without_camera_has_none() {
        assert!(load(&fixture("cube_nocam.glb")).unwrap().camera.is_none());
    }

    #[test]
    fn textured_glb_keeps_its_texture() {
        let s = load(&fixture("textured.glb")).unwrap();
        let t = s.materials[s.primitives[0].material].texture.unwrap();
        assert_eq!(s.textures[t].get_pixel(0, 0).0, [0, 255, 0, 255]);
    }

    #[test]
    fn truncated_glb_is_err() {
        let dir = tempfile::tempdir().unwrap();
        let bytes = std::fs::read(fixture("cube.glb")).unwrap();
        let p = dir.path().join("cut.glb");
        std::fs::write(&p, &bytes[..bytes.len() / 2]).unwrap();
        assert!(matches!(load(&p), Err(ModelError::Parse(_) | ModelError::Io(_))));
    }

    #[test]
    fn gltf_with_missing_bin_is_err() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("cube.gltf");
        std::fs::copy(fixture("cube.gltf"), &p).unwrap();
        assert!(matches!(load(&p), Err(ModelError::Parse(_) | ModelError::Io(_))));
    }

    /// Splits cube.glb into (json, bin), lets `edit` change them, writes the result.
    fn patched_glb(
        dir: &std::path::Path,
        edit: impl FnOnce(&mut serde_json::Value, &mut Vec<u8>),
    ) -> std::path::PathBuf {
        let b = std::fs::read(fixture("cube.glb")).unwrap();
        let jlen = u32::from_le_bytes(b[12..16].try_into().unwrap()) as usize;
        let mut json: serde_json::Value = serde_json::from_slice(&b[20..20 + jlen]).unwrap();
        let bin_start = 20 + jlen + 8;
        let mut bin = b[bin_start..].to_vec();
        edit(&mut json, &mut bin);
        let mut j = serde_json::to_vec(&json).unwrap();
        while !j.len().is_multiple_of(4) {
            j.push(b' ');
        }
        let mut out = b"glTF".to_vec();
        out.extend(2u32.to_le_bytes());
        out.extend(((12 + 8 + j.len() + 8 + bin.len()) as u32).to_le_bytes());
        out.extend((j.len() as u32).to_le_bytes());
        out.extend(b"JSON");
        out.extend(&j);
        out.extend((bin.len() as u32).to_le_bytes());
        out.extend(b"BIN ");
        out.extend(&bin);
        let p = dir.join("patched.glb");
        std::fs::write(&p, out).unwrap();
        p
    }

    #[test]
    fn out_of_range_index_is_parse_error() {
        let dir = tempfile::tempdir().unwrap();
        let p = patched_glb(dir.path(), |json, bin| {
            let acc = json["meshes"][0]["primitives"][0]["indices"].as_u64().unwrap() as usize;
            let a = &json["accessors"][acc];
            let view = &json["bufferViews"][a["bufferView"].as_u64().unwrap() as usize];
            let off = view["byteOffset"].as_u64().unwrap_or(0) as usize
                + a["byteOffset"].as_u64().unwrap_or(0) as usize;
            let width = if a["componentType"] == 5123 { 2 } else { 4 };
            bin[off..off + width].fill(0xFF);
        });
        assert!(matches!(load(&p), Err(ModelError::Parse(_))));
    }

    #[test]
    fn zero_scale_camera_is_dropped() {
        let dir = tempfile::tempdir().unwrap();
        let p = patched_glb(dir.path(), |json, _| {
            for n in json["nodes"].as_array_mut().unwrap() {
                if n.get("camera").is_some() {
                    n["scale"] = serde_json::json!([0.0, 0.0, 0.0]);
                }
            }
        });
        let s = load(&p).unwrap();
        assert!(s.camera.is_none());
        assert_eq!(s.triangle_count(), 12);
    }

    #[test]
    fn empty_scene_is_empty_error() {
        assert!(matches!(load(&fixture("empty.glb")), Err(ModelError::Empty)));
    }
}

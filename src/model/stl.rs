use std::fs::File;
use std::io::BufReader;
use std::path::Path;

use super::{DEFAULT_COLOR, Material, ModelError, Primitive, Scene, UpAxis, Vertex};

pub(super) fn load_stl(path: &Path) -> Result<Scene, ModelError> {
    let mut reader = BufReader::new(File::open(path)?);
    let mut vertices = Vec::new();
    for tri in stl_io::create_stl_reader(&mut reader)? {
        let tri = tri?;
        // Unshared vertices keep flat shading once normals are filled.
        for v in tri.vertices {
            vertices.push(Vertex {
                position: v.into(),
                normal: [0.0; 3],
                uv: [0.0; 2],
            });
        }
    }
    let count =
        u32::try_from(vertices.len()).map_err(|_| ModelError::Parse("too many vertices".into()))?;
    let mut scene = Scene {
        vertices,
        indices: (0..count).collect(),
        primitives: vec![Primitive {
            indices: 0..count,
            material: 0,
        }],
        materials: vec![Material {
            base_color: DEFAULT_COLOR,
            texture: None,
        }],
        textures: Vec::new(),
        camera: None,
        up: UpAxis::Z,
    };
    scene.fill_missing_normals();
    Ok(scene)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn triangle() -> stl_io::Triangle {
        stl_io::Triangle {
            normal: stl_io::Normal::new([0.0, 0.0, 1.0]),
            vertices: [
                stl_io::Vertex::new([0.0, 0.0, 0.0]),
                stl_io::Vertex::new([1.0, 0.0, 0.0]),
                stl_io::Vertex::new([0.0, 1.0, 0.0]),
            ],
        }
    }

    fn write_binary(path: &Path) {
        let mut file = File::create(path).unwrap();
        stl_io::write_stl(&mut file, [triangle()].iter()).unwrap();
    }

    #[test]
    fn stl_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.stl");
        write_binary(&path);
        let scene = load_stl(&path).unwrap();
        assert_eq!(scene.triangle_count(), 1);
        assert_eq!(scene.up, UpAxis::Z);
        assert_eq!(scene.materials[0].base_color, DEFAULT_COLOR);
        assert_eq!(scene.vertices[0].normal, [0.0, 0.0, 1.0]);
    }

    #[test]
    fn ascii_and_binary_stl_both_load() {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("bin.stl");
        write_binary(&bin);
        let ascii = dir.path().join("ascii.stl");
        std::fs::write(
            &ascii,
            "solid t\nfacet normal 0 0 1\nouter loop\nvertex 0 0 0\nvertex 1 0 0\nvertex 0 1 0\nendloop\nendfacet\nendsolid t\n",
        )
        .unwrap();
        assert_eq!(load_stl(&bin).unwrap().triangle_count(), 1);
        assert_eq!(load_stl(&ascii).unwrap().triangle_count(), 1);
    }

    #[test]
    fn truncated_stl_is_err() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.stl");
        write_binary(&path);
        let bytes = std::fs::read(&path).unwrap();
        std::fs::write(&path, &bytes[..bytes.len() - 20]).unwrap();
        assert!(load_stl(&path).is_err());
    }
}

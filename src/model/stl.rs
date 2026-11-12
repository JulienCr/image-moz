use std::fs::File;
use std::io::{BufReader, Cursor, Read, Seek};
use std::path::Path;

use super::{DEFAULT_COLOR, Material, ModelError, Primitive, Scene, UpAxis, Vertex};

const HEADER_LEN: usize = 80;

/// stl_io takes any file starting with "solid " for ASCII, but SolidWorks writes binary ones that
/// do; a length that matches the binary layout settles it.
fn binary_with_solid_header(file: &mut File) -> std::io::Result<bool> {
    let len = file.metadata()?.len();
    let mut head = [0u8; HEADER_LEN + 4];
    let read = file.read(&mut head)?;
    file.rewind()?;
    let triangles = u64::from(u32::from_le_bytes(head[HEADER_LEN..].try_into().unwrap()));
    Ok(read == head.len() && head.starts_with(b"solid") && len == head.len() as u64 + 50 * triangles)
}

fn read_vertices<R: Read + Seek>(reader: &mut R) -> Result<Vec<Vertex>, ModelError> {
    let mut vertices = Vec::new();
    for tri in stl_io::create_stl_reader(reader)? {
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
    Ok(vertices)
}

pub(super) fn load_stl(path: &Path) -> Result<Scene, ModelError> {
    let mut file = File::open(path)?;
    let vertices = if binary_with_solid_header(&mut file)? {
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)?;
        bytes[..HEADER_LEN].fill(0);
        read_vertices(&mut Cursor::new(bytes))?
    } else {
        read_vertices(&mut BufReader::new(file))?
    };
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
    fn binary_stl_with_a_solid_header_loads_as_binary() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.stl");
        let mut file = File::create(&path).unwrap();
        // 10 triangles put a newline byte right after the header, which fools the ASCII probe.
        stl_io::write_stl(&mut file, vec![triangle(); 10].iter()).unwrap();
        drop(file);
        let mut bytes = std::fs::read(&path).unwrap();
        bytes[..80].fill(b' ');
        bytes[..14].copy_from_slice(b"solid exported");
        std::fs::write(&path, bytes).unwrap();
        assert_eq!(load_stl(&path).unwrap().triangle_count(), 10);
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

use std::collections::HashMap;
use std::fs::File;
use std::io::BufReader;
use std::path::Path;

use glam::{Mat4, Vec3};

use super::{DEFAULT_COLOR, Material, ModelError, Primitive, Scene, UpAxis, Vertex};

const MAX_COMPONENT_DEPTH: usize = 16;
/// Ceiling on component visits plus instantiated mesh vertices, so a fan-out cannot exhaust memory.
const MAX_INSTANCE_COST: usize = 20_000_000;

struct Builder<'a> {
    objects: HashMap<usize, &'a threemf::model::Object>,
    scene: Scene,
}

/// Cost of an object at a nesting depth, shared by every path that reaches it.
type CostMemo = HashMap<(usize, usize), usize>;

impl<'a> Builder<'a> {
    /// Visits plus mesh vertices an object expands to, without instantiating anything.
    fn cost(&self, id: usize, depth: usize, memo: &mut CostMemo) -> Result<usize, ModelError> {
        if depth > MAX_COMPONENT_DEPTH {
            return Err(ModelError::Parse("3MF components nested too deeply".into()));
        }
        if let Some(&known) = memo.get(&(id, depth)) {
            return Ok(known);
        }
        let object = self.object(id)?;
        let mut total = 1 + object.mesh.as_ref().map_or(0, |m| m.vertices.vertex.len());
        for c in object.components.iter().flat_map(|c| &c.component) {
            total = total.saturating_add(self.cost(c.objectid, depth + 1, memo)?);
        }
        memo.insert((id, depth), total);
        Ok(total)
    }

    fn object(&self, id: usize) -> Result<&'a threemf::model::Object, ModelError> {
        self.objects
            .get(&id)
            .copied()
            .ok_or_else(|| ModelError::Parse(format!("3MF references missing object {id}")))
    }

    /// Callers have checked the depth and cost with `cost`.
    fn add_object(&mut self, id: usize, transform: Mat4) -> Result<(), ModelError> {
        let object = self.object(id)?;
        if let Some(mesh) = &object.mesh {
            self.add_mesh(mesh, transform)?;
        }
        for c in object.components.iter().flat_map(|c| &c.component) {
            let local = c.transform.map_or(Mat4::IDENTITY, matrix);
            self.add_object(c.objectid, transform * local)?;
        }
        Ok(())
    }

    fn add_mesh(&mut self, mesh: &threemf::Mesh, transform: Mat4) -> Result<(), ModelError> {
        let too_many = || ModelError::Parse("too many vertices".into());
        let base = u32::try_from(self.scene.vertices.len()).map_err(|_| too_many())?;
        let count = mesh.vertices.vertex.len();
        let mut indices = Vec::with_capacity(mesh.triangles.triangle.len() * 3);
        for t in &mesh.triangles.triangle {
            for i in [t.v1, t.v2, t.v3] {
                if i >= count {
                    return Err(ModelError::Parse("3MF triangle index out of range".into()));
                }
                indices.push(base + u32::try_from(i).map_err(|_| too_many())?);
            }
        }
        for v in &mesh.vertices.vertex {
            let p = transform.transform_point3(Vec3::new(v.x as f32, v.y as f32, v.z as f32));
            self.scene.vertices.push(Vertex {
                position: p.to_array(),
                normal: [0.0; 3],
                uv: [0.0; 2],
            });
        }
        let start = self.scene.indices.len() as u32;
        let end = start + indices.len() as u32;
        self.scene.indices.extend(indices);
        self.scene.primitives.push(Primitive {
            indices: start..end,
            material: 0,
        });
        Ok(())
    }
}

/// 3MF stores a row-vector 4x3 matrix, which is glam's column order read as-is.
fn matrix(m: [f64; 12]) -> Mat4 {
    let f = |i: usize| m[i] as f32;
    Mat4::from_cols_array(&[
        f(0),
        f(1),
        f(2),
        0.0,
        f(3),
        f(4),
        f(5),
        0.0,
        f(6),
        f(7),
        f(8),
        0.0,
        f(9),
        f(10),
        f(11),
        1.0,
    ])
}

pub(super) fn load_3mf(path: &Path) -> Result<Scene, ModelError> {
    let models = threemf::read(BufReader::new(File::open(path)?))
        .map_err(|e| ModelError::Parse(e.to_string()))?;

    let mut builder = Builder {
        objects: HashMap::new(),
        scene: Scene {
            vertices: Vec::new(),
            indices: Vec::new(),
            primitives: Vec::new(),
            materials: vec![Material {
                base_color: DEFAULT_COLOR,
                texture: None,
            }],
            textures: Vec::new(),
            camera: None,
            up: UpAxis::Z,
        },
    };
    for model in &models {
        for object in &model.resources.object {
            builder.objects.insert(object.id, object);
        }
    }
    let (mut memo, mut total) = (CostMemo::new(), 0usize);
    for model in &models {
        for item in &model.build.item {
            total = total.saturating_add(builder.cost(item.objectid, 0, &mut memo)?);
        }
    }
    if total > MAX_INSTANCE_COST {
        return Err(ModelError::Parse("3MF expands to too much geometry".into()));
    }
    for model in &models {
        for item in &model.build.item {
            let transform = item.transform.map_or(Mat4::IDENTITY, matrix);
            builder.add_object(item.objectid, transform)?;
        }
    }

    let mut scene = builder.scene;
    scene.fill_missing_normals();
    Ok(scene)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn triangle_mesh() -> threemf::Mesh {
        let v = |x, y, z| threemf::model::Vertex { x, y, z };
        threemf::Mesh {
            vertices: threemf::model::Vertices {
                vertex: vec![v(0.0, 0.0, 0.0), v(1.0, 0.0, 0.0), v(0.0, 1.0, 0.0)],
            },
            triangles: threemf::model::Triangles {
                triangle: vec![threemf::model::Triangle {
                    v1: 0,
                    v2: 1,
                    v3: 2,
                }],
            },
        }
    }

    fn write_3mf(path: &Path) {
        threemf::write(File::create(path).unwrap(), triangle_mesh()).unwrap();
    }

    #[test]
    fn threemf_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.3mf");
        write_3mf(&path);
        let scene = load_3mf(&path).unwrap();
        assert_eq!(scene.triangle_count(), 1);
        assert_eq!(scene.up, UpAxis::Z);
        assert_eq!(scene.materials[0].base_color, DEFAULT_COLOR);
    }

    #[test]
    fn truncated_3mf_is_err() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.3mf");
        write_3mf(&path);
        let bytes = std::fs::read(&path).unwrap();
        std::fs::write(&path, &bytes[..bytes.len() / 2]).unwrap();
        assert!(load_3mf(&path).is_err());
    }

    /// A chain of `levels` objects, each listing the next `fan_out` times; the last holds a mesh.
    fn nested_model(levels: usize, fan_out: usize) -> threemf::model::Model {
        let mut model = threemf::model::Model::from(triangle_mesh());
        let leaf = model.resources.object.pop().unwrap();
        let object = |id, components: Vec<usize>| threemf::model::Object {
            id,
            partnumber: None,
            name: None,
            pid: None,
            pindex: None,
            mesh: None,
            components: Some(threemf::model::Components {
                component: components
                    .into_iter()
                    .map(|objectid| threemf::model::Component { objectid, transform: None })
                    .collect(),
            }),
        };
        let leaf_id = levels + 1;
        for id in 1..=levels {
            model.resources.object.push(object(id, vec![id + 1; fan_out]));
        }
        model.resources.object.push(threemf::model::Object { id: leaf_id, ..leaf });
        model.build.item[0].objectid = 1;
        model
    }

    fn write_model(dir: &Path, model: threemf::model::Model) -> std::path::PathBuf {
        let path = dir.join("t.3mf");
        threemf::write(File::create(&path).unwrap(), model).unwrap();
        path
    }

    #[test]
    fn moderate_nesting_loads() {
        let dir = tempfile::tempdir().unwrap();
        let scene = load_3mf(&write_model(dir.path(), nested_model(4, 2))).unwrap();
        assert_eq!(scene.triangle_count(), 16);
    }

    #[test]
    fn exponential_fan_out_is_a_parse_error_without_instantiating() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_model(dir.path(), nested_model(MAX_COMPONENT_DEPTH, 4));
        let start = std::time::Instant::now();
        assert!(matches!(load_3mf(&path), Err(ModelError::Parse(m)) if m.contains("too much geometry")));
        assert!(start.elapsed() < std::time::Duration::from_secs(2));
    }

    #[test]
    fn component_cycle_is_a_parse_error() {
        let dir = tempfile::tempdir().unwrap();
        let mut model = nested_model(1, 1);
        model.resources.object[1].mesh = None;
        model.resources.object[0].components.as_mut().unwrap().component[0].objectid = 1;
        assert!(matches!(load_3mf(&write_model(dir.path(), model)), Err(ModelError::Parse(_))));
    }

    #[test]
    fn build_item_transform_applies() {
        let mut model = threemf::model::Model::from(triangle_mesh());
        // Translate by (10, 0, 0).
        model.build.item[0].transform =
            Some([1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 10.0, 0.0, 0.0]);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.3mf");
        threemf::write(File::create(&path).unwrap(), model).unwrap();

        let scene = load_3mf(&path).unwrap();
        assert_eq!(scene.vertices[0].position, [10.0, 0.0, 0.0]);
    }
}

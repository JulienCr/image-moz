//! Renderable 3D scene shared by every model loader.

pub mod camera;
mod gltf;
pub mod gpu;
mod obj;
pub mod offscreen;
mod stl;
mod threemf;

use std::ops::Range;
use std::path::Path;

use bytemuck::{Pod, Zeroable};
use glam::Vec3;

pub const DEFAULT_COLOR: [f32; 4] = [0.8, 0.8, 0.8, 1.0];

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct Vertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub uv: [f32; 2],
}

/// Linear base color; `texture` indexes `Scene::textures`.
pub struct Material {
    pub base_color: [f32; 4],
    pub texture: Option<usize>,
}

/// Range into `Scene::indices`.
pub struct Primitive {
    pub indices: Range<u32>,
    pub material: usize,
}

pub struct SceneCamera {
    pub eye: Vec3,
    pub target: Vec3,
    pub up: Vec3,
    pub yfov: f32,
    /// Width over height the file camera was authored for.
    pub aspect: Option<f32>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum UpAxis {
    Y,
    Z,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Aabb {
    pub min: Vec3,
    pub max: Vec3,
}

impl Aabb {
    pub fn center(&self) -> Vec3 {
        (self.min + self.max) * 0.5
    }

    /// Half the diagonal.
    pub fn radius(&self) -> f32 {
        (self.max - self.min).length() * 0.5
    }
}

pub struct Scene {
    pub vertices: Vec<Vertex>,
    pub indices: Vec<u32>,
    pub primitives: Vec<Primitive>,
    pub materials: Vec<Material>,
    pub textures: Vec<image::RgbaImage>,
    pub camera: Option<SceneCamera>,
    pub up: UpAxis,
}

impl Scene {
    pub fn triangle_count(&self) -> u32 {
        self.indices.len() as u32 / 3
    }

    pub fn bounds(&self) -> Option<Aabb> {
        let mut it = self.vertices.iter().map(|v| Vec3::from(v.position));
        let first = it.next()?;
        let (min, max) = it.fold((first, first), |(lo, hi), p| (lo.min(p), hi.max(p)));
        Some(Aabb { min, max })
    }

    /// Replaces zero normals by the normal of the face they belong to.
    pub fn fill_missing_normals(&mut self) {
        for tri in self.indices.chunks_exact(3) {
            let [a, b, c] = [tri[0] as usize, tri[1] as usize, tri[2] as usize];
            let p = |i: usize| Vec3::from(self.vertices[i].position);
            let n = (p(b) - p(a)).cross(p(c) - p(a)).normalize_or_zero();
            for i in [a, b, c] {
                if Vec3::from(self.vertices[i].normal) == Vec3::ZERO {
                    self.vertices[i].normal = n.to_array();
                }
            }
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ModelError {
    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),
    #[error("invalid model: {0}")]
    Parse(String),
    #[error("model contains no triangles")]
    Empty,
    #[error("unsupported model format")]
    Unsupported,
}

pub fn load(path: &Path) -> Result<Scene, ModelError> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    let scene = match ext.as_str() {
        "glb" | "gltf" => gltf::load_gltf(path)?,
        "obj" => obj::load_obj(path)?,
        "stl" => stl::load_stl(path)?,
        "3mf" => threemf::load_3mf(path)?,
        _ => return Err(ModelError::Unsupported),
    };
    if scene.triangle_count() == 0 {
        return Err(ModelError::Empty);
    }
    Ok(scene)
}

#[cfg(test)]
pub(crate) fn fixture(name: &str) -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/3d")
        .join(name)
}

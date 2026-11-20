use galaxy_evidence::FrameData;

use super::{Pose, Rng, CX, CY, FX, FY, HEIGHT, WIDTH};
use super::Scenario;

const PIXELS: usize = WIDTH * HEIGHT;
type Vec3 = [f64; 3];

#[derive(Clone, Copy)]
enum Shape {
    Plane { z: f64, slope: f64 },
    Box { lo: Vec3, hi: Vec3 },
    Sphere { center: Vec3, radius: f64 },
    CurvedScreen { z: f64, curvature: f64 },
}

#[derive(Clone, Copy)]
enum Material {
    Pigment { id: u8, dark: bool },
    Picture { half_x: f64, half_y: f64 },
}

struct Object { shape: Shape, material: Material }

struct Ray { o: Vec3, d: Vec3, inv: Vec3 }

impl Ray {
    fn at(&self, t: f64) -> Vec3 {
        std::array::from_fn(|i| self.o[i] + t * self.d[i])
    }
}

fn dot(a: Vec3, b: Vec3) -> f64 { a.into_iter().zip(b).map(|(a, b)| a * b).sum() }

fn first_root(a: f64, b: f64, c: f64) -> f64 {
    if a.abs() < 1e-15 { return -c / b; }
    let disc = b * b - 4.0 * a * c;
    if disc < 0.0 { return f64::INFINITY; }
    let t = -0.5 * (b + libm::sqrt(disc).copysign(b));
    let (r0, r1) = (t / a, c / t);
    if r0 > 0.0 && (r1 <= 0.0 || r0 < r1) { r0 } else { r1 }
}

impl Shape {
    fn intersect(self, ray: &Ray) -> f64 {
        match self {
            Self::Plane { z, slope } => (z + slope * ray.o[0] - ray.o[2]) / (ray.d[2] - slope * ray.d[0]),
            Self::Box { lo, hi } => {
                let mut enter: f64 = 0.0;
                let mut leave = f64::INFINITY;
                for i in 0..3 {
                    if ray.d[i] == 0.0 {
                        if !(lo[i]..=hi[i]).contains(&ray.o[i]) { return f64::INFINITY; }
                        continue;
                    }
                    let s = (lo[i] - ray.o[i]) * ray.inv[i];
                    let t = (hi[i] - ray.o[i]) * ray.inv[i];
                    enter = enter.max(s.min(t));
                    leave = leave.min(s.max(t));
                }
                if leave < enter { f64::INFINITY } else if enter > 0.0 { enter } else { leave }
            }
            Self::Sphere { center, radius } => {
                let o: Vec3 = std::array::from_fn(|i| ray.o[i] - center[i]);
                first_root(dot(ray.d, ray.d), 2.0 * dot(o, ray.d), dot(o, o) - radius * radius)
            }
            Self::CurvedScreen { z, curvature } => {
                let a = curvature * ray.d[0] * ray.d[0];
                let b = 2.0 * curvature * ray.o[0] * ray.d[0] - ray.d[2];
                let c = z + curvature * ray.o[0] * ray.o[0] - ray.o[2];
                first_root(a, b, c)
            }
        }
    }
}

fn pigment(shape: Shape, id: u8, at: Vec3) -> [u8; 3] {
    const COLORS: [[u8; 3]; 10] = [
        [183, 154, 119], [117, 148, 156], [214, 138, 78], [90, 134, 78], [154, 90, 65],
        [76, 109, 162], [201, 194, 178], [169, 127, 155], [63, 86, 105], [208, 183, 104],
    ];
    let [x, y, z] = at;
    let phase = f64::from(id) * 0.137;
    let grid = |u: f64| libm::floor(u) as i32;
    let checker = (grid(x * 14.0 + y * 3.0 + phase) ^ grid(y * 14.0 + z * 5.0 + phase)) & 1;
    let fine = (grid(53.0 * x + 8.0 * z + phase) ^ grid(47.0 * y + 12.0 * z)) & 1;
    let grain = if matches!(shape, Shape::Box { .. }) { 17 } else { 12 };
    let brightness = (2 * checker - 1) * grain + (2 * fine - 1) * 7;
    COLORS[usize::from(id) % COLORS.len()].map(|x| (i32::from(x) + brightness).clamp(0, 255) as u8)
}

fn box_at(center: Vec3, half: Vec3, id: u8, dark: bool) -> Object {
    Object {
        shape: Shape::Box {
            lo: std::array::from_fn(|i| center[i] - half[i]),
            hi: std::array::from_fn(|i| center[i] + half[i]),
        },
        material: Material::Pigment { id, dark },
    }
}

fn room(seed: u64) -> Vec<Object> {
    let mut rng = Rng(seed);
    let mut objects = Vec::with_capacity(21);
    // Back wall, tabletop, side of cabinet. Broad surfaces at different
    // angles ensure this is not merely a depth-noise or variance test.
    objects.push(Object { shape: Shape::Plane { z: 3.95, slope: 0.14 }, material: Material::Pigment { id: 6, dark: false } });
    objects.push(box_at([0.0, 0.63, 2.5], [1.4, 0.045, 1.65], 0, false));
    objects.push(box_at([-1.16, -0.36, 2.75], [0.23, 1.1, 0.8], 1, false));
    for (row, y) in [-0.34, -0.07, 0.20].into_iter().enumerate() {
        for (col, x) in [-0.61, -0.35, -0.10, 0.16, 0.42, 0.66].into_iter().enumerate() {
            let r = 0.103 + 0.038 * rng.unit();
            let z = 1.17 + 1.45 * rng.unit();
            let center = [(x + 0.075 * rng.signed()) * z, (y + 0.075 * rng.signed()) * z, z];
            let id = (row * 6 + col + 2) as u8;
            if (row + col) % 3 == 0 {
                objects.push(Object { shape: Shape::Sphere { center, radius: r * z }, material: Material::Pigment { id, dark: false } });
            } else {
                objects.push(box_at(center, [r * z, (r + 0.04 * rng.signed()) * z, 0.17 + 0.10 * rng.unit()], id, false));
            }
        }
    }
    objects
}

struct Hit { ray_depth: f64, index: usize, rgb: [u8; 3], dark: bool }

fn raycast(ray: &Ray, objects: &[Object], picture: Option<&[u8]>) -> Hit {
    let mut best = Hit { ray_depth: f64::INFINITY, index: usize::MAX, rgb: [30, 30, 30], dark: true };
    for (i, object) in objects.iter().enumerate() {
        let depth = object.shape.intersect(ray);
        if depth <= 0.08 || depth >= best.ray_depth || !depth.is_finite() { continue; }
        let at = ray.at(depth);
        let (rgb, dark) = match object.material {
            Material::Pigment { id, dark } => (pigment(object.shape, id, at), dark),
            Material::Picture { half_x, half_y } => {
                if at[0].abs() >= half_x || at[1].abs() >= half_y { continue; }
                let u = ((at[0] / half_x + 1.0) * 0.5 * WIDTH as f64) as usize;
                let v = ((at[1] / half_y + 1.0) * 0.5 * HEIGHT as f64) as usize;
                let k = 3 * (WIDTH * v.min(HEIGHT - 1) + u.min(WIDTH - 1));
                let pixels = picture.expect("a picture material has a rendered texture");
                ([pixels[k], pixels[k + 1], pixels[k + 2]], false)
            }
        };
        best = Hit { ray_depth: depth, index: i, rgb, dark };
    }
    best
}

fn rays(pose: Pose) -> impl Iterator<Item = Ray> {
    let matrix = pose.matrix();
    (0..PIXELS).map(move |i| {
        let uv = [((i % WIDTH) as f64 - CX) / FX, ((i / WIDTH) as f64 - CY) / FY, 1.0];
        let d = std::array::from_fn(|n| matrix[3 * n] * uv[0] + matrix[3 * n + 1] * uv[1] + matrix[3 * n + 2]);
        Ray { o: pose.t, d, inv: d.map(|x| x.recip()) }
    })
}

pub(super) fn render(scenario: Scenario, seed: u64, pose: Pose, frame: usize) -> FrameData {
    let is_monitor = matches!(scenario, Scenario::FrontalMonitor | Scenario::TiltedMonitor | Scenario::CurvedMonitor);
    let has_image = is_monitor || matches!(scenario, Scenario::Print | Scenario::CentreMonitor);
    let walls_only = is_monitor || matches!(scenario, Scenario::FlatWall | Scenario::Print);
    let mut objects = if walls_only {
        vec![Object { shape: Shape::Plane { z: 2.88, slope: 0.0 }, material: Material::Pigment { id: 6, dark: false } }]
    } else { room(seed) };
    if has_image {
        let (z, half_x, half_y) = match scenario {
            Scenario::Print => (2.8795, 0.75, 0.51),
            Scenario::CentreMonitor => (0.96, 0.39, 0.37),
            _ => (1.55, 1.40, 1.10),
        };
        let shape = if scenario == Scenario::CurvedMonitor {
            Shape::CurvedScreen { z, curvature: -0.23 }
        } else {
            Shape::Plane { z, slope: if scenario == Scenario::TiltedMonitor { 0.33 } else { 0.0 } }
        };
        objects.push(Object { shape, material: Material::Picture { half_x, half_y } });
    }
    if scenario == Scenario::Reflective {
        objects.push(box_at([0.09, 0.04, 0.95], [0.39, 0.31, 0.04], 5, true));
    }
    if scenario == Scenario::Dynamic {
        objects.push(box_at([(frame as f64 - 1.0) * 0.43, -0.02, 0.77], [0.32, 0.32, 0.05], 8, false));
    }
    let picture: Option<Vec<u8>> = has_image.then(|| {
        let room = room(seed ^ 0xe098_3253);
        rays(Pose::default()).flat_map(|ray| raycast(&ray, &room, None).rgb).collect()
    });
    let hits: Vec<_> = rays(pose).map(|ray| raycast(&ray, &objects, picture.as_deref())).collect();
    let mut rng = Rng(seed ^ 0x2a86_7641_1b47_333c ^ frame as u64);
    let depth_bias = 0.007 * rng.normal();
    let mut out = FrameData { rgb: Vec::with_capacity(3 * PIXELS), depth_mm: Vec::with_capacity(PIXELS), conf: Vec::with_capacity(PIXELS) };
    for (i, hit) in hits.iter().enumerate() {
        let d = hit.ray_depth;
        let (x, y) = (i % WIDTH, i / WIDTH);
        let is_edge = [
            (x > 0).then(|| i - 1), (x + 1 < WIDTH).then(|| i + 1),
            (y > 0).then(|| i - WIDTH), (y + 1 < HEIGHT).then(|| i + WIDTH),
        ].into_iter().flatten().any(|j| {
            let other = &hits[j];
            // A material boundary without true depth separation (e.g. print)
            // is not a LiDAR step.
            other.index != hit.index && (other.ray_depth - d).abs() > 0.02 * d
        });
        let dark = hit.dark || hit.rgb.iter().copied().max().unwrap_or(0) < 56;
        let dropout = if hit.dark { 0.88 } else if d > 4.2 { 0.22 } else if is_edge || dark { 0.045 } else { 0.002 };
        let n = rng.normal();
        let missing = !d.is_finite() || rng.unit() < dropout;
        out.depth_mm.push(if missing { 0 } else { libm::round(d * (1.0 + depth_bias + 0.007 * n) * 1000.0) as u16 });
        out.conf.push(if missing || hit.dark { 0 } else if d > 3.5 || is_edge || dark { 1 } else { 2 });
        out.rgb.extend(if scenario == Scenario::LowTexture { [127; 3] } else { hit.rgb });
    }
    out
}

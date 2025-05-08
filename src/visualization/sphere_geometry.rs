const GOLDEN_RATIO_CONJUGATE: f32 = 0.61803398875; // (sqrt(5) - 1) / 2

pub fn generate_sphere_points_fibonacci(radius: f32, num_points: usize) -> Vec<[f32; 3]> {
    let mut points = Vec::with_capacity(num_points);
    for i in 0..num_points {
        let y = 1.0 - (i as f32 / (num_points - 1) as f32) * 2.0; // `y` has a range of 1 to -1
        let r = (1.0 - y * y).sqrt(); // radius at y
        let theta = (i as f32 * GOLDEN_RATIO_CONJUGATE) * std::f32::consts::TAU; // tau is 2*PI

        let x = (theta.cos() * r) * radius;
        let z = (theta.sin() * r) * radius;
        points.push([x, y * radius, z]);
    }

    // Return vec of 3D points representing locations on a sphere's surface
    points
}

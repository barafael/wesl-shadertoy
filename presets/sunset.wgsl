// Sunset — https://www.shadertoy.com/view/wlBXWK by Dimas Leenman (MIT)
// Ported from eliotbo/bevy_shadertoy_wgsl.
import package::shadertoy::i_time;
import package::shadertoy::i_resolution;
import package::shadertoy::i_mouse;

// Divides i_time: higher is a slower day.
const TIME_SCALE: f32 = 3.0; // tweak: 0.5..12
// Sunlight strength.
const LIGHT_INTENSITY: vec3<f32> = vec3<f32>(40.); // tweak: 0..100
// Rayleigh scattering coefficients (what makes the sky blue).
const BETA_RAY: vec3<f32> = vec3<f32>(0.0000055, 0.000013, 0.0000224);
// Mie scattering coefficient (haze around the sun).
const BETA_MIE: vec3<f32> = vec3<f32>(0.000021);
// Mie anisotropy: how forward the haze scatters.
const MIE_G: f32 = 0.7; // tweak: 0..0.99
const GROUND_COLOR: vec3<f32> = vec3<f32>(0., 0.25, 0.05); // tweak: color
// Ray march steps along the view ray and towards the sun.
const STEPS_PRIMARY: i32 = 12; // tweak: 1..32
const STEPS_LIGHT: i32 = 4; // tweak: 1..16

fn calculate_scattering(start: vec3<f32>, dir: vec3<f32>, max_dist: f32, scene_color: vec3<f32>, light_dir: vec3<f32>, light_intensity: vec3<f32>, planet_position: vec3<f32>, planet_radius: f32, atmo_radius: f32, beta_ray: vec3<f32>, beta_mie: vec3<f32>, beta_absorption: vec3<f32>, beta_ambient: vec3<f32>, g: f32, height_ray: f32, height_mie: f32, height_absorption: f32, absorption_falloff: f32, steps_i: i32, steps_l: i32) -> vec3<f32> {
    var start_var = start;
    start_var = start_var - (planet_position);
    var a: f32 = dot(dir, dir);
    var b: f32 = 2. * dot(dir, start_var);
    var c: f32 = dot(start_var, start_var) - atmo_radius * atmo_radius;
    var d: f32 = b * b - 4. * a * c;
    if (d < 0.) {    return scene_color;
}
    var ray_length: vec2<f32> = vec2<f32>(max((-b - sqrt(d)) / (2. * a), 0.), min((-b + sqrt(d)) / (2. * a), max_dist));
    if (ray_length.x > ray_length.y) {    return scene_color;
}
    var allow_mie: bool = max_dist > ray_length.y;
    ray_length.y = min(ray_length.y, max_dist);
    ray_length.x = max(ray_length.x, 0.);
    let step_size_i: f32 = (ray_length.y - ray_length.x) / f32(steps_i);
    var ray_pos_i: f32 = ray_length.x + step_size_i * 0.5;
    var total_ray: vec3<f32> = vec3<f32>(0.);
    var total_mie: vec3<f32> = vec3<f32>(0.);
    var opt_i: vec3<f32> = vec3<f32>(0.);
    let scale_height: vec2<f32> = vec2<f32>(height_ray, height_mie);
    var mu: f32 = dot(dir, light_dir);
    var mumu: f32 = mu * mu;
    var gg: f32 = g * g;
    let phase_ray: f32 = 3. / 50.265484 * (1. + mumu);
    var phase_mie: f32 = 0.;
    if (allow_mie) {
        phase_mie = 3. / 25.132742 * ((1. - gg) * (mumu + 1.)) / (pow(1. + gg - 2. * mu * g, 1.5) * (2. + gg));
    } else {
        phase_mie = 0.;
        };

    for (var i: i32 = 0; i < steps_i; i = i + 1) {
        let pos_i: vec3<f32> = start_var + dir * ray_pos_i;
        let height_i: f32 = length(pos_i) - planet_radius;
        var density: vec3<f32> = vec3<f32>(exp(-height_i / scale_height), 0.);
        var denom: f32 = (height_absorption - height_i) / absorption_falloff;
        density.z = 1. / (denom * denom + 1.) * density.x;
        density = density * (step_size_i);
        opt_i = opt_i + (density);
        a = dot(light_dir, light_dir);
        b = 2. * dot(light_dir, pos_i);
        c = dot(pos_i, pos_i) - atmo_radius * atmo_radius;
        d = b * b - 4. * a * c;
        let step_size_l: f32 = (-b + sqrt(d)) / (2. * a * f32(steps_l));
        var ray_pos_l: f32 = step_size_l * 0.5;
        var opt_l: vec3<f32> = vec3<f32>(0.);

        for (var l: i32 = 0; l < steps_l; l = l + 1) {
            let pos_l: vec3<f32> = pos_i + light_dir * ray_pos_l;
            let height_l: f32 = length(pos_l) - planet_radius;
            var density_l: vec3<f32> = vec3<f32>(exp(-height_l / scale_height), 0.);
            let denom: f32 = (height_absorption - height_l) / absorption_falloff;
            density_l.z = 1. / (denom * denom + 1.) * density_l.x;
            density_l = density_l * (step_size_l);
            opt_l = opt_l + (density_l);
            ray_pos_l = ray_pos_l + (step_size_l);
        }

        let attn: vec3<f32> = exp(-beta_ray * (opt_i.x + opt_l.x) - beta_mie * (opt_i.y + opt_l.y) - beta_absorption * (opt_i.z + opt_l.z));
        total_ray = total_ray + (density.x * attn);
        total_mie = total_mie + (density.y * attn);
        ray_pos_i = ray_pos_i + (step_size_i);
    }

    let opacity: vec3<f32> = exp(-(beta_mie * opt_i.y + beta_ray * opt_i.x + beta_absorption * opt_i.z));
    return (phase_ray * beta_ray * total_ray + phase_mie * beta_mie * total_mie + opt_i.x * beta_ambient) * light_intensity + scene_color * opacity;
}

fn ray_sphere_intersect(start: vec3<f32>, dir: vec3<f32>, radius: f32) -> vec2<f32> {
    let a: f32 = dot(dir, dir);
    let b: f32 = 2. * dot(dir, start);
    let c: f32 = dot(start, start) - radius * radius;
    let d: f32 = b * b - 4. * a * c;
    if (d < 0.) {    return vec2<f32>(100000., -100000.);
}
    return vec2<f32>((-b - sqrt(d)) / (2. * a), (-b + sqrt(d)) / (2. * a));
}

fn skylight(sample_pos: vec3<f32>, surface_normal: vec3<f32>, light_dir: vec3<f32>, background_col: vec3<f32>) -> vec3<f32> {
    var surface_normal_var = surface_normal;
    surface_normal_var = normalize(mix(surface_normal_var, light_dir, 0.6));
    return calculate_scattering(sample_pos, surface_normal_var, 3. * 6471000., background_col, light_dir, LIGHT_INTENSITY, vec3<f32>(0.), 6371000., 6471000., BETA_RAY, BETA_MIE, vec3<f32>(0.0000204, 0.0000497, 0.00000195), vec3<f32>(0.), MIE_G, 8000., 1200., 30000., 4000., 4, 4);
}

fn render_scene(pos: vec3<f32>, dir: vec3<f32>, light_dir: vec3<f32>) -> vec4<f32> {
    var color: vec4<f32> = vec4<f32>(0., 0., 0., 1000000000000.);
    var colorxyz = color.xyz;
    // colorxyz = vec3<f32>;
    if (dot(dir, light_dir) > 0.9998) {
        colorxyz = vec3<f32>(3.);
    } else {
        colorxyz =vec3<f32>( 0.);
        };
    color.x = colorxyz.x;
    color.y = colorxyz.y;
    color.z = colorxyz.z;
    let planet_intersect: vec2<f32> = ray_sphere_intersect(pos - vec3<f32>(0.), dir, 6371000.);
    if (0. < planet_intersect.y) {
        color.w = max(planet_intersect.x, 0.);
        let sample_pos: vec3<f32> = pos + dir * planet_intersect.x - vec3<f32>(0.);
        let surface_normal: vec3<f32> = normalize(sample_pos);
        var colorxyz = color.xyz;
    colorxyz = GROUND_COLOR;
    color.x = colorxyz.x;
    color.y = colorxyz.y;
    color.z = colorxyz.z;
        let N: vec3<f32> = surface_normal;
        let V: vec3<f32> = -dir;
        let L: vec3<f32> = light_dir;
        let dotNV: f32 = max(0.000001, dot(N, V));
        let dotNL: f32 = max(0.000001, dot(N, L));
        let shadow: f32 = dotNL / (dotNL + dotNV);
        var colorxyz_2 = color.xyz;
    colorxyz_2 = color.xyz * (shadow);
    color.x = colorxyz_2.x;
    color.y = colorxyz_2.y;
    color.z = colorxyz_2.z;
        var colorxyz_3 = color.xyz;
    colorxyz_3 = color.xyz + (clamp(skylight(sample_pos, surface_normal, light_dir, vec3<f32>(0.)) * GROUND_COLOR, vec3<f32>(0.), vec3<f32>(1.)));
    color.x = colorxyz_3.x;
    color.y = colorxyz_3.y;
    color.z = colorxyz_3.z;
    }
    return color;
}

fn get_camera_vector(resolution: vec2<f32>, coord: vec2<f32>) -> vec3<f32> {
    var uv: vec2<f32> = coord.xy / resolution.xy - vec2<f32>(0.5);
    uv.x = uv.x * (resolution.x / resolution.y);
    return normalize(vec3<f32>(uv.x, uv.y, -1.));
}

fn main_image(frag_coord: vec2<f32>) -> vec4<f32> {
    let time = i_time() / TIME_SCALE;

    let camera_vector: vec3<f32> = get_camera_vector(i_resolution().xy, frag_coord);
    let offset: f32 = (1. - cos(time / 2.)) * 6471000.;
    let camera_position: vec3<f32> = vec3<f32>(0., 6371000. + 1., offset);
    var light_dir: vec3<f32> = vec3<f32>(0.);
    if (i_mouse().y == 0.) {
        light_dir = normalize(vec3<f32>(0., cos(-time / 8.), sin(-time / 8.)));
    } else {
        light_dir = normalize(vec3<f32>(0., cos(i_mouse().y * -5. / i_resolution().y), sin(i_mouse().y * -5. / i_resolution().y)));
    };
    let scene: vec4<f32> = render_scene(camera_position, camera_vector, light_dir);
    var col: vec3<f32> = vec3<f32>(0.);
    col = col + (calculate_scattering(camera_position, camera_vector, scene.w, scene.xyz, light_dir, LIGHT_INTENSITY, vec3<f32>(0.), 6371000., 6471000., BETA_RAY, BETA_MIE, vec3<f32>(0.0000204, 0.0000497, 0.00000195), vec3<f32>(0.), MIE_G, 8000., 1200., 30000., 4000., STEPS_PRIMARY, STEPS_LIGHT));
    col = 1. - exp(-col);

    return vec4<f32>((col), 1.);
}

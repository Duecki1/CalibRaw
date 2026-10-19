#import calibraw::common as Common

fn presence_reference_scale() -> f32 {
    return Common::resolution_scale(0.55, 3.0);
}

fn soft_detail_threshold(detail: f32, threshold: f32) -> f32 {
    return sign(detail) * max(abs(detail) - threshold, 0.0);
}

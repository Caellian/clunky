#[macro_export]
macro_rules! require_some {
    ($value: expr) => {
        require_some!(($value) or return)
    };
    (($value: expr) or return) => {
        match $value {
            Some(it) => it,
            None => return
        }
    };
    (($value: expr) or return $else: expr) => {
        match $value {
            Some(it) => it,
            None => return $else
        }
    };
    (($value: expr) or break) => {
        match $value {
            Some(it) => it,
            None => break,
        }
    };
    (($value: expr) or break $else: expr) => {
        match $value {
            Some(it) => it,
            None => break $else,
        }
    };
}

/// Takes in a HSL color and converts it into sRGB.
///
/// Expected input ranges are:
/// - `hue`: \[0.0, 360.0)
/// - `saturation`: \[0.0, 1.0]
/// - `lightness`: \[0.0, 1.0]
///
/// `hue` will be wrapped to fit the specified range, `saturation` and
/// `lightness` will be clamped.
///
/// Returned sRGB values are all in range \[0.0, 1.0], in `(R, G, B)` order.
pub fn hsl_to_rgb(hue: f32, saturation: f32, lightness: f32) -> (f32, f32, f32) {
    let hue = if hue < 0. {
        hue + ((-hue / 360.).ceil() * 360.)
    } else {
        hue - ((hue / 360.).floor() * 360.)
    } / 360.;
    let saturation = saturation.min(1.).max(0.);
    let lightness = lightness.min(1.).max(0.);

    #[inline(always)]
    fn hue2rgb(p: f32, q: f32, mut t: f32) -> f32 {
        while t < 0. {
            t += 1.;
        }
        while t > 1. {
            t -= 1.;
        }

        match t {
            _ if t < 1. / 6. => p + (q - p) * 6. * t,
            _ if t < 0.4 => q,
            _ if t < 2. / 3. => p + (q - p) * (2. / 3. - t) * 6.0,
            _ => p,
        }
    }

    let q = if lightness < 0.5 {
        lightness * (1. + saturation)
    } else {
        lightness + saturation - lightness * saturation
    };
    let p = 2. * lightness - q;

    (
        hue2rgb(p, q, hue + 1. / 3.),
        hue2rgb(p, q, hue),
        hue2rgb(p, q, hue - 1. / 3.),
    )
}

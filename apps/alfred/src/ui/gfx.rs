//! The orb's compositor: each `rune::effect` slot the view places becomes one rune-fx layer on the GPU.
//!
//! The orb is water in a glass ball, as slots stacked in one spot. A soft green glow sits behind it. The
//! body of water is deep at the edges and lit from the top left, so it reads as a sphere. Light veins
//! flow, fold and twist over it (warped noise with one bright band, screened on), like sunlight moving
//! through water. A ripple over both makes the whole orb shimmer, and the glass ball on top has a
//! highlight that drifts like a moving surface.
//!
//! While listening, rings ripple out from its edge and the water turns a brighter mint. Each slot's
//! `effect_data` carries `[energy, level, mode, 0]`: energy (0..1) speeds the water and brightens it;
//! level (the voice) brightens the veins and the glow and strengthens the ripple.

use rune::compose::{Compositor, DeviceKey, Frame};
use rune::render::{EffectPhase, EffectSite, KIND_EFFECT};
use rune_fx::*;

use super::Ui;

pub const GLOW: u32 = 1;
pub const BODY: u32 = 2;
pub const VEINS: u32 = 3;
pub const LENS: u32 = 4;
/// Listening only: rings rippling out from the orb's edge.
pub const RINGS: u32 = 5;
pub const RIPPLE: u32 = 6;

/// Mode codes in `effect_data[2]`.
pub const LISTENING: f32 = 1.0;
pub const THINKING: f32 = 3.0;
pub const SPEAKING: f32 = 4.0;

fn is(code: f32, mode: f32) -> bool {
    (code - mode).abs() < 0.5
}

/// The body, from the light near the top left to the deep at the edge.
const BODY_GREENS: [u32; 4] = [0x8ff7d6, 0x16a47d, 0x05553f, 0x011a14];
const BODY_LISTENING: [u32; 4] = [0xc8fff4, 0x22c3b0, 0x0a6b62, 0x022925];
/// The veins: black (no light) with one bright band, screened over the body.
const VEIN_RAMP: [u32; 6] = [0x000000, 0x000000, 0x0d5c48, 0xb8ffe6, 0x0d5c48, 0x000000];

/// The layer for one slot (its kind is the slot's mark; its data, its rect in points, the time), and how it
/// blends onto what is beneath.
pub fn layer(kind: u32, d: [f32; 4], rect: [f32; 4], t: f32) -> Option<(FxLayer, FxBlend)> {
    let [x, y, w, h] = rect;
    let r = w.min(h) / 2.0;
    let (cx, cy) = (x + w / 2.0, y + h / 2.0);
    let e = d[0].clamp(0.0, 1.0);
    let level = d[1].clamp(0.0, 1.0);
    let (listening, thinking, speaking) = (is(d[2], LISTENING), is(d[2], THINKING), is(d[2], SPEAKING));
    let circle = Shape::circle(cx, cy, r);
    let mut blend = FxBlend::Normal;
    let fx = match kind {
        GLOW => Fx::glow(GlowParams {
            style: GlowStyle::Soft,
            color: rgb(0x10b981),
            core: rgb(0xa7f3d0),
            radius: glow_radius(e, level),
            intensity: 0.5 + 0.8 * e + if speaking { 0.6 * level } else { 0.0 },
            breathe: if thinking { 0.6 } else { 0.3 },
            breathe_period: if thinking { 1.2 } else { 3.6 },
            ..Default::default()
        }),
        BODY => Fx::fill(GrainGradientParams {
            kind: GradientKind::Radial,
            colors: Stops::hex(if listening { &BODY_LISTENING } else { &BODY_GREENS }),
            center: [0.36, 0.30],
            amount: 0.03,
            ..GrainGradientParams::default()
        }),
        VEINS => {
            blend = FxBlend::Screen;
            Fx::fill(
                FillParams::new(FillStyle::Swirl(SwirlParams {
                    colors: Stops::hex(&VEIN_RAMP),
                    scale: r * 0.9,
                    speed: 0.3 + if thinking { 1.3 } else { 0.6 } * e,
                    twist: if thinking { 1.4 } else { 0.5 + 0.4 * e },
                    warp: 1.4 + 0.5 * e,
                }))
                .intensity(0.55 + 0.35 * e + 0.3 * level),
            )
        }
        RIPPLE => Fx::distort(DistortParams::HeatHaze {
            height: h,
            scale: 30.0,
            speed: 0.8 + 1.0 * e,
            amount: 1.8 + 2.4 * e + 3.5 * level,
            whole_shape: true,
        }),
        LENS => Fx::lens(LensParams {
            bevel: r * 0.22,
            depth: r * 0.26,
            dispersion: 0.12,
            magnification: 1.05,
            frost: 0.04,
            rim: 1.9,
            shadow: 0.0,
            light_angle: -0.85 + 0.3 * (t * 0.6).sin(),
            tint: rgb(0xa7f3d0),
            tint_amount: 0.04,
            ..Default::default()
        }),
        RINGS => Fx::pulse(PulseParams {
            style: PulseStyle::Radar,
            color: rgb(0x5eead4),
            core: rgb(0xf0fdfa),
            count: 3,
            period: 1.6 - 0.6 * level,
            reach: ring_reach(level),
            width: 3.0,
            intensity: 1.0 + 0.6 * level,
            ..Default::default()
        }),
        _ => return None,
    };
    Some((fx.on(circle).at(t), blend))
}

/// How far the glow reaches past the orb's edge, in points: at most 32.
pub fn glow_radius(energy: f32, level: f32) -> f32 {
    14.0 + 10.0 * energy.clamp(0.0, 1.0) + 8.0 * level.clamp(0.0, 1.0)
}

/// How far the listening rings travel past the orb's edge, in points: at most 32, so with the orb at its
/// largest they still end inside the orb's space instead of being cut by the window.
pub fn ring_reach(level: f32) -> f32 {
    14.0 + 18.0 * level.clamp(0.0, 1.0)
}

struct FxCache {
    device_key: DeviceKey,
    format: wgpu::TextureFormat,
    fx: FxRenderer,
}

#[derive(Default)]
pub struct Gfx {
    fx: Option<FxCache>,
}

impl Compositor<Ui> for Gfx {
    fn animating(&self) -> bool {
        true
    }

    fn render(&mut self, f: &mut Frame<'_, Ui>) {
        let (device, queue, size) = (f.device, f.queue, f.size);
        let t = f.rt.t as f32;
        let marks = &f.rt.marks;
        let mut at: Vec<u32> = Vec::new();
        let mut layers: Vec<FxLayer> = Vec::new();
        let mut placements: Vec<FxSlot> = Vec::new();
        for (n, i) in f.rt.instances.iter().enumerate() {
            if i.params[1] != KIND_EFFECT {
                continue;
            }
            let kind = marks.get(n).copied().unwrap_or(0) & 0xff;
            let k = i.params[3];
            let rect = i.rect.map(|v| v / k);
            let Some((l, blend)) = layer(kind, i.uv, rect, t) else { continue };
            at.push(n as u32);
            layers.push(l);
            placements.push(FxSlot::default().scale(k).clip(i.clip, i.fade).blend(blend));
        }
        let device_key = f.device_key();
        if self.fx.as_ref().is_none_or(|c| c.device_key != device_key || c.format != f.format) {
            self.fx = None;
            let mut fx = FxRenderer::new(device, f.format, FxOptions { dither: true });
            fx.add_format(device, wgpu::TextureFormat::Rgba16Float);
            self.fx = Some(FxCache { device_key, format: f.format, fx });
        }
        let Some(cache) = self.fx.as_mut() else { return };
        let fx = &mut cache.fx;
        fx.set_srgb_blend(f.srgb_blend());
        fx.prepare_with_slots(
            device,
            queue,
            &FxFrame { viewport: [size.0, size.1], scale: 1.0, layers: &layers, backdrop: None, sources: &[] },
            &placements,
        );
        let clear = f.rt.clear_color();
        f.ui.prepare(device, queue, &mut f.rt.text.atlas, &f.rt.instances, size);
        f.ui.draw_with_backdrop(f.encoder, f.target, clear, &mut |phase, s: &EffectSite<'_>| {
            let Some(k) = at.iter().position(|n| *n == s.index) else { return };
            match phase {
                EffectPhase::Backdrop(enc) => {
                    if let Some(bd) = s.backdrop {
                        fx.blur_backdrop(device, queue, enc, bd, k);
                    }
                }
                EffectPhase::Draw(pass) => fx.draw_layer_in(pass, s.format, s.origin, k),
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_orb_slot_has_a_layer_and_unknown_marks_none() {
        for kind in [GLOW, BODY, VEINS, RIPPLE, LENS, RINGS] {
            assert!(layer(kind, [0.5, 0.2, 0.0, 0.0], [0.0, 0.0, 150.0, 150.0], 1.0).is_some(), "kind {kind}");
        }
        assert!(layer(99, [0.0; 4], [0.0, 0.0, 10.0, 10.0], 0.0).is_none());
    }
}

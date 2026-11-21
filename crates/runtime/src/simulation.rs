use std::collections::{HashMap, HashSet};

use shader_graph::codegen::{RenderPlan, ResourceDecl};
use shader_graph::passes::{Size, StateRead, TextureFormat};

use crate::clock::FrameInputs;
use crate::formats::wgpu_format;

/// A simulation's last step: the frame it was drawn for, and the seconds it stepped by.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StepRecord {
    pub frame: u64,
    pub time: f64,
    pub delta: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StepKind {
    /// Start over from the initial values.
    Start,
    /// Advance from the previous frame's state.
    Advance,
    /// Compute the frame it last did again (e.g. after a change while paused)
    /// from the same state.
    Repeat,
}

/// What a simulation does on a frame, and the seconds it steps by.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Step {
    pub kind: StepKind,
    pub delta: f64,
}

/// How a simulation that last stepped for `last` (if it has) goes on to `frame`.
pub fn next_step(last: Option<StepRecord>, frame: FrameInputs) -> Step {
    let Some(last) = last.filter(|l| frame.frame != 0 && frame.frame >= l.frame) else {
        return Step { kind: StepKind::Start, delta: 0.0 };
    };
    if frame.frame == last.frame {
        return Step { kind: StepKind::Repeat, delta: last.delta };
    }
    // Frames may have been skipped: the step covers all the time since the last.
    Step { kind: StepKind::Advance, delta: (frame.time - last.time).max(0.0) }
}

/// The two textures keeping a state item, which swap roles as frames go by.
struct Pair {
    format: TextureFormat,
    simulation: String,
    textures: [wgpu::Texture; 2],
    /// The index of this frame's texture; the other is the previous frame's.
    current: usize,
}

/// The state of a plan's simulations kept between the frames drawn to one
/// target: each state item's texture of the previous frame and of this one,
/// and how each simulation last stepped. It outlives plans: a new one keeps
/// the state of the simulations it shares with the last, as long as their
/// items keep their formats and sizes.
#[derive(Default)]
pub struct SimulationState {
    pairs: HashMap<String, Pair>,
    steps: HashMap<String, StepRecord>,
}

impl SimulationState {
    /// Begins drawing `frame` of `plan`: makes the textures it keeps state in
    /// where they're missing or don't fit (their simulations then start
    /// over), and steps each simulation. Returns their steps, by id.
    /// `size_of` gives the size a state texture is to have, by resource id.
    pub fn advance(
        &mut self,
        device: &wgpu::Device,
        plan: &RenderPlan,
        frame: FrameInputs,
        size_of: &mut dyn FnMut(&str) -> Size,
    ) -> HashMap<String, Step> {
        let mut fresh = HashSet::new();
        let mut used = HashSet::new();
        let mut simulations = HashSet::new();
        for resource in &plan.resources {
            let ResourceDecl::State { id, texture, format, simulation, .. } = resource else { continue };
            used.insert(texture.clone());
            simulations.insert(simulation.clone());
            let size = size_of(id);
            let fits = self.pairs.get(texture).is_some_and(|pair| {
                pair.format == *format
                    && pair.simulation == *simulation
                    && pair.textures[0].width() == size.width
                    && pair.textures[0].height() == size.height
            });
            if fits {
                continue;
            }
            let make = || create_texture(device, *format, size);
            self.pairs.insert(
                texture.clone(),
                Pair {
                    format: *format,
                    simulation: simulation.clone(),
                    textures: [make(), make()],
                    current: 0,
                },
            );
            fresh.insert(simulation.clone());
        }
        self.pairs.retain(|id, _| used.contains(id));
        self.steps.retain(|id, _| simulations.contains(id));

        let mut steps = HashMap::new();
        for id in simulations {
            let step = if fresh.contains(&id) {
                Step { kind: StepKind::Start, delta: 0.0 }
            } else {
                next_step(self.steps.get(&id).copied(), frame)
            };
            if step.kind == StepKind::Advance {
                for pair in self.pairs.values_mut().filter(|p| p.simulation == id) {
                    pair.current = 1 - pair.current;
                }
            }
            self.steps.insert(id.clone(), StepRecord { frame: frame.frame, time: frame.time, delta: step.delta });
            steps.insert(id, step);
        }
        steps
    }

    /// The texture keeping a state item (by `state_texture_id`), as of `read`'s frame.
    pub fn texture(&self, texture: &str, read: StateRead) -> Option<&wgpu::Texture> {
        let pair = self.pairs.get(texture)?;
        pair.textures.get(if read == StateRead::Current { pair.current } else { 1 - pair.current })
    }

    /// Forgets every simulation's state: they all start over on the next frame.
    pub fn reset(&mut self) {
        self.pairs.clear();
        self.steps.clear();
    }
}

fn create_texture(device: &wgpu::Device, format: TextureFormat, size: Size) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("simulation state"),
        size: wgpu::Extent3d { width: size.width, height: size.height, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu_format(format),
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(frame: u64, time: f64) -> FrameInputs {
        FrameInputs { time, delta: 0.0, frame }
    }

    #[test]
    fn starts_without_a_last_step_and_on_the_first_frame() {
        assert_eq!(next_step(None, frame(5, 1.0)).kind, StepKind::Start);
        let last = StepRecord { frame: 3, time: 0.5, delta: 0.1 };
        assert_eq!(next_step(Some(last), frame(0, 0.0)).kind, StepKind::Start);
    }

    #[test]
    fn starts_over_when_time_goes_back() {
        let last = StepRecord { frame: 10, time: 1.0, delta: 0.1 };
        assert_eq!(next_step(Some(last), frame(4, 0.4)).kind, StepKind::Start);
    }

    #[test]
    fn repeats_the_same_frame_with_its_step() {
        let last = StepRecord { frame: 10, time: 1.0, delta: 0.25 };
        assert_eq!(next_step(Some(last), frame(10, 1.0)), Step { kind: StepKind::Repeat, delta: 0.25 });
    }

    #[test]
    fn advances_over_skipped_frames() {
        let last = StepRecord { frame: 10, time: 1.0, delta: 0.1 };
        let step = next_step(Some(last), frame(13, 1.5));
        assert_eq!(step.kind, StepKind::Advance);
        assert!((step.delta - 0.5).abs() < 1e-12);
    }
}

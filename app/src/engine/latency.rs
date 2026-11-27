use super::{Plan, StepKind};

/// How a processor passes delay from its inputs to its outputs.
pub enum NodeLatency {
    Process { samples: u32, generates_audio: bool },
    PassThrough { generates_audio: bool },
    Silent,
}

struct Route {
    id: u32,
    inputs: Vec<Vec<usize>>,
    outputs: Vec<usize>,
}

/// The UI keeps just the buffer connections from the compiled audio plan.
#[derive(Default)]
pub struct LatencyRoutes {
    steps: Vec<Route>,
    device_inputs: Vec<usize>,
    output_sources: Vec<usize>,
    buffer_count: usize,
}

impl Plan {
    pub fn latency_routes(&self) -> LatencyRoutes {
        let output_sources: Vec<_> = self.dev_out.iter().flat_map(|(sources, _)| sources.iter().copied()).collect();
        let buffer_count = self.pool.len() / self.max_block;
        let mut feeds_output = vec![false; buffer_count];
        for &source in &output_sources {
            feeds_output[source] = true;
        }
        let mut steps = Vec::new();
        for step in self.steps.iter().rev() {
            if step.kind == StepKind::Proc && step.outs.iter().any(|&out| feeds_output[out]) {
                for &source in step.sources.iter().flatten() {
                    feeds_output[source] = true;
                }
                steps.push(Route { id: step.cell.id, inputs: step.sources.clone(), outputs: step.outs.clone() });
            }
        }
        steps.reverse();
        LatencyRoutes { steps, device_inputs: self.dev_in.clone(), output_sources, buffer_count }
    }
}

impl LatencyRoutes {
    /// Parallel inputs arrive independently. The latest one determines the output delay.
    pub fn output_samples(&self, mut latency: impl FnMut(u32) -> NodeLatency) -> u64 {
        // A connected zero-delay signal is different from a cable with no source behind it.
        let mut delays = vec![None; self.buffer_count];
        for &input in &self.device_inputs {
            delays[input] = Some(0);
        }
        for step in &self.steps {
            match latency(step.id) {
                NodeLatency::Process { samples, generates_audio } => {
                    let input = step.inputs.iter().flatten().filter_map(|&buffer| delays[buffer]).max().or(generates_audio.then_some(0));
                    for &out in &step.outputs {
                        delays[out] = input.map(|delay| delay + u64::from(samples));
                    }
                }
                NodeLatency::PassThrough { generates_audio } => {
                    for (&out, sources) in step.outputs.iter().zip(&step.inputs) {
                        delays[out] = sources.iter().filter_map(|&buffer| delays[buffer]).max().or(generates_audio.then_some(0));
                    }
                }
                NodeLatency::Silent => {}
            }
        }
        self.output_sources.iter().filter_map(|&buffer| delays[buffer]).max().unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::{GraphDesc, NodeCell, NodeDesc, MIDI_PIN};

    fn node(id: u32, kind: StepKind) -> NodeDesc {
        NodeDesc { cell: NodeCell::new(id, None), kind, ins: 2, outs: 2 }
    }

    fn effect(samples: u32) -> NodeLatency {
        NodeLatency::Process { samples, generates_audio: false }
    }

    #[test]
    fn adds_serial_delays_and_takes_the_longest_connected_audio_path() {
        let g = GraphDesc {
            nodes: vec![
                node(8, StepKind::AudioIn),
                node(9, StepKind::AudioOut),
                node(3, StepKind::Proc),
                node(2, StepKind::Proc),
                node(1, StepKind::Proc),
                node(4, StepKind::Proc),
                node(5, StepKind::Proc),
            ],
            // Stereo serial connection, parallel input, a control cable, and an inactive output.
            conns: vec![(8, 0, 1, 0), (8, 1, 2, 1), (1, 0, 3, 0), (1, 1, 3, 1), (2, 0, 3, 0), (3, 0, 9, 0), (4, MIDI_PIN, 3, MIDI_PIN), (5, 0, 9, 1)],
            ..Default::default()
        };
        let routes = Plan::build(&g, 64, 2, 1).latency_routes();
        let samples = routes.output_samples(|id| effect(match id {
            1 => 128,
            2 => 256,
            3 => 64,
            _ => panic!("Polled a plug-in outside the output path"),
        }));
        assert_eq!(samples, 320);
        let no_output = Plan::build(&g, 64, 2, 0).latency_routes();
        assert_eq!(no_output.output_samples(|_| panic!("No output connected")), 0);
    }

    #[test]
    fn bypass_follows_the_matching_channel_and_adds_no_delay() {
        let g = GraphDesc {
            nodes: vec![node(0, StepKind::AudioIn), node(1, StepKind::Proc), node(2, StepKind::Proc), node(3, StepKind::Proc), node(4, StepKind::AudioOut)],
            conns: vec![(0, 0, 1, 0), (0, 0, 2, 0), (1, 0, 3, 0), (2, 0, 3, 1), (3, 1, 4, 0)],
            ..Default::default()
        };
        let routes = Plan::build(&g, 64, 2, 1).latency_routes();
        let measure = |bypassed| routes.output_samples(|id| match id {
            1 => effect(512),
            2 => effect(128),
            _ if bypassed => NodeLatency::PassThrough { generates_audio: false },
            _ => effect(64),
        });
        assert_eq!(measure(false), 576);
        assert_eq!(measure(true), 128);
    }

    #[test]
    fn rerouting_an_effect_only_counts_delay_from_a_connected_source() {
        let mut g = GraphDesc {
            nodes: vec![node(1, StepKind::AudioIn), node(2, StepKind::Proc), node(3, StepKind::Proc), node(4, StepKind::AudioOut)],
            conns: vec![(1, 0, 4, 0), (2, 0, 3, 0), (3, 0, 4, 0)],
            ..Default::default()
        };
        let routes = |graph: &GraphDesc| Plan::build(graph, 64, 1, 1).latency_routes();
        let disconnected = routes(&g);
        assert_eq!(disconnected.output_samples(|_| effect(128)), 0);
        assert_eq!(disconnected.output_samples(|id| NodeLatency::Process { samples: 128, generates_audio: id == 2 }), 256);
        g.conns.push((1, 0, 2, 0));
        let connected = routes(&g);
        assert_eq!(connected.output_samples(|_| effect(128)), 256);
        assert_eq!(connected.output_samples(|id| effect(if id == 3 { 512 } else { 128 })), 640);
        g.conns.retain(|c| c.0 != 3);
        assert_eq!(routes(&g).output_samples(|_| effect(128)), 0);
    }
}

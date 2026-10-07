//! Naive scalar reference interpreter over a [`FlatCircuit`].
//!
//! One bool per net, one lane, no bit-slicing. This is the oracle the
//! bit-sliced evaluator is tested against, and — because it keeps *every*
//! net (the folding pass never runs here) — the source of truth for
//! live-signal probing via [`FlatSimulator::net_value`]. It is not meant
//! for a realtime thread.

use std::borrow::Cow;

use crate::compile::{FlatCircuit, NetId};

#[derive(Debug, Clone)]
pub struct FlatSimulator<'a> {
    circuit: Cow<'a, FlatCircuit>,
    /// Combinational snapshot of the most recent tick, per net.
    values: Vec<bool>,
    regs: Vec<bool>,
    inputs: Vec<bool>,
}

impl<'a> FlatSimulator<'a> {
    /// Borrows an existing circuit.
    pub fn new(circuit: &'a FlatCircuit) -> Self {
        Self::with_circuit(Cow::Borrowed(circuit))
    }

    fn with_circuit(circuit: Cow<'a, FlatCircuit>) -> Self {
        let net_count = circuit.net_count as usize;
        let regs = circuit.regs.iter().map(|reg| reg.init).collect();
        let inputs = vec![false; circuit.inputs.len()];
        Self {
            values: vec![false; net_count],
            regs,
            inputs,
            circuit,
        }
    }

    /// Resets every register to its `init` value.
    pub fn reset(&mut self) {
        for (value, reg) in self.regs.iter_mut().zip(self.circuit.regs.iter()) {
            *value = reg.init;
        }
    }

    /// Sets a top-level input by port name. Returns `false` when the
    /// port does not exist.
    #[must_use]
    pub fn set_input(&mut self, name: &str, value: bool) -> bool {
        match self
            .circuit
            .inputs
            .iter()
            .position(|(port, _)| port == name)
        {
            Some(index) => {
                self.inputs[index] = value;
                true
            }
            None => false,
        }
    }

    /// Runs one tick: evaluate combinationally, then latch registers.
    pub fn tick(&mut self) {
        for (value, (_, net)) in self.inputs.iter().zip(self.circuit.inputs.iter()) {
            self.values[*net as usize] = *value;
        }
        for (value, reg) in self.regs.iter().zip(self.circuit.regs.iter()) {
            self.values[reg.q as usize] = *value;
        }
        for gate in &self.circuit.gates {
            self.values[gate.y as usize] =
                !(self.values[gate.a as usize] && self.values[gate.b as usize]);
        }
        for (value, reg) in self.regs.iter_mut().zip(self.circuit.regs.iter()) {
            *value = self.values[reg.d as usize];
        }
    }

    /// Reads a top-level output by port name, as of the last tick.
    pub fn output(&self, name: &str) -> Option<bool> {
        self.circuit
            .outputs
            .iter()
            .find(|(port, _)| port == name)
            .map(|(_, net)| self.values[*net as usize])
    }

    /// The number of nets (valid [`NetId`]s are `0..net_count`).
    pub fn net_count(&self) -> usize {
        self.circuit.net_count as usize
    }

    /// The combinational value of net `net` as of the last [`tick`], or
    /// `None` if `net` is out of range.
    ///
    /// Register timing: after a tick, `net_value(reg.q)` is the value the
    /// register *presented during* that tick, and `net_value(reg.d)` is
    /// the value latched at its end; the newly latched value only appears
    /// on `q` on the next tick. Net values are not meaningful until at
    /// least one tick has run (`new` / `reset` do not settle logic).
    ///
    /// [`tick`]: FlatSimulator::tick
    pub fn net_value(&self, net: NetId) -> Option<bool> {
        self.values.get(net as usize).copied()
    }

    /// The full per-net value snapshot from the last tick.
    pub fn values(&self) -> &[bool] {
        &self.values
    }
}

impl FlatSimulator<'static> {
    /// Takes ownership of a circuit, so the simulator can outlive the
    /// scope that produced it (e.g. a GUI-held probe runner) without a
    /// self-referential borrow.
    pub fn from_owned(circuit: FlatCircuit) -> Self {
        Self::with_circuit(Cow::Owned(circuit))
    }
}

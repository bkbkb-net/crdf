//! Real-time evaluation of a [`CompiledCircuit`] — generic, with no
//! domain assumptions.
//!
//! Evaluation is bit-sliced: lane `k` of every `u64` word is instance
//! `k` of the circuit, so one instruction pass advances up to 64
//! independent instances of the same program. One tick is: load
//! register outputs and cached inputs into the signal scratch, run the
//! instructions in topological order, then latch every register input
//! simultaneously (double buffer). One *step* is `ticks_per_step`
//! ticks; outputs are the combinational snapshot of the final tick.
//!
//! Inputs come in two flavours per call site, not per declaration:
//! **levels** persist until changed ([`CircuitState::set_level`]),
//! **pulses** ([`CircuitState::pulse`]) are asserted for tick 0 of the
//! next step only and then clear themselves — the general form of a
//! strobe/edge input.
//!
//! `CircuitState` performs no allocation, locking or I/O inside
//! [`CircuitState::process_step`]; allocate it off the real-time
//! thread.

use crate::compile::CompiledCircuit;

pub const LANES: usize = 64;

/// Mutable evaluation state for one group of up to 64 lanes. Owned
/// exclusively by the processing thread; must only be used with the
/// program that created it.
#[derive(Debug, Clone)]
pub struct CircuitState {
    program_id: uuid::Uuid,
    /// Combinational scratch, one `u64` per slot. Register `q` slots
    /// occupy the first `regs` words.
    signals: Vec<u64>,
    reg_current: Vec<u64>,
    reg_next: Vec<u64>,
    /// Cached level inputs, parallel to the compiled input bindings.
    level_lanes: Vec<u64>,
    /// One-step pulse inputs, parallel to the compiled input bindings;
    /// asserted for tick 0 of the next step, then cleared.
    pulse_lanes: Vec<u64>,
    /// Persistent lane-major state for regions a JIT substituted and
    /// whose state nothing outside them reads
    /// (`crate::compile::CompiledRegion::state_is_private`). Grown to
    /// fit whichever program is running; empty until one needs it, and
    /// meaningless to anything but the lowering that wrote it.
    ///
    /// The register array stays authoritative for everything else, so
    /// the interpreter and a JIT without lowerings both work off it
    /// exactly as before.
    lane_state: Vec<u64>,
    /// Set once a program that keeps private state has run.
    ///
    /// From then on the register array is not the whole truth: some
    /// registers live only in `lane_state`, in a layout only the
    /// lowering understands. The interpreter reads the array, so it
    /// would carry on from values that stopped being current -- quietly
    /// and audibly. Refused instead.
    private_state_is_live: bool,
    /// Where each private-state region sits in `lane_state`, remembered
    /// from the program that last ran. Kept here so resetting a lane
    /// clears that state too, wherever the reset comes from -- the
    /// caller resetting a lane has a `CircuitState` and need not also
    /// have the program.
    lane_state_layout: Option<Vec<crate::lane_state::LaneStateBinding>>,
    /// Ring storage of each recognised shift chain, and where each
    /// ring is currently being written. Heads are shared by all lanes,
    /// which run in lockstep.
    rings: Vec<Vec<u64>>,
    heads: Vec<u32>,
    /// Output words captured at the final tick of the last step.
    output_words: Vec<u64>,
}

impl CircuitState {
    pub fn new(circuit: &CompiledCircuit) -> Self {
        let mut state = Self {
            program_id: circuit.program_id,
            signals: vec![0; circuit.slot_count as usize],
            lane_state: Vec::new(),
            private_state_is_live: false,
            lane_state_layout: None,
            reg_current: vec![0; circuit.regs.len()],
            reg_next: vec![0; circuit.regs.len()],
            level_lanes: vec![0; circuit.inputs.len()],
            pulse_lanes: vec![0; circuit.inputs.len()],
            rings: circuit
                .banks
                .iter()
                .map(|bank| vec![0; bank.len as usize])
                .collect(),
            heads: vec![0; circuit.banks.len()],
            output_words: vec![0; circuit.outputs.len()],
        };
        for lane in 0..LANES {
            state.reset_lane_registers(circuit, lane);
        }
        state
    }

    fn check_program(&self, circuit: &CompiledCircuit) {
        assert_eq!(
            self.program_id, circuit.program_id,
            "CircuitState used with a different CompiledCircuit"
        );
    }

    /// Resets one lane's registers to their `init` values (instance
    /// restart). Level and pulse inputs are left untouched; use
    /// Puts every lane back to the state a fresh one starts in, and
    /// makes the backends interchangeable again.
    ///
    /// After this both representations hold nothing but `init`, so the
    /// register array is once more the whole truth and the interpreter
    /// may take over. That is why the flag is cleared here and only
    /// here: a *lane* reset leaves the other sixty-three alone, so the
    /// private buffer still holds state the register array does not.
    pub fn reset_all_lanes(&mut self, circuit: &CompiledCircuit) {
        self.check_program(circuit);
        for lane in 0..LANES {
            self.reset_lane_registers(circuit, lane);
        }
        self.lane_state.fill(0);
        // And the binding: with both representations back to `init`
        // there is nothing left for a differently laid-out program to
        // land on top of, so refusing one would be refusing nothing.
        self.lane_state_layout = None;
        self.private_state_is_live = false;
    }
    /// [`Self::clear_lane_inputs`] for a full lane reset.
    pub fn reset_lane_registers(&mut self, circuit: &CompiledCircuit, lane: usize) {
        self.check_program(circuit);
        assert!(lane < LANES);
        let mask = 1_u64 << lane;
        for (word, spec) in self.reg_current.iter_mut().zip(circuit.regs.iter()) {
            if spec.init {
                *word |= mask;
            } else {
                *word &= !mask;
            }
        }
        // A bank holds the state of the registers it replaced, so the
        // same reset has to reach every entry of its ring. The head is
        // deliberately left alone: it is shared by all sixty-four lanes
        // running in lockstep, and moving it would disturb the other
        // sixty-three. Filling the whole ring makes this lane's entries
        // identical, so the head's phase stops mattering for it.
        for (ring, spec) in self.rings.iter_mut().zip(circuit.banks.iter()) {
            for word in ring.iter_mut() {
                if spec.init {
                    *word |= mask;
                } else {
                    *word &= !mask;
                }
            }
        }
        // And whatever a lowering is keeping privately for this lane.
        //
        // The layout is byte-granular: lane `l` owns `k` whole bytes,
        // where `k` is `ceil(registers / 8)`. Every register eligible
        // for private state starts at zero, so clearing those bytes
        // *is* the reset, whatever the lowering reads them as -- and
        // zero is zero whichever way round the bytes go.
        //
        // Done here rather than in a method of its own because a method
        // of its own is a method somebody has to remember to call, and
        // the first version of this was exactly that: a lane reset
        // cleared the register array and left a region's private state
        // untouched, so the lane's next use inherited the last one's --
        // and only in the accelerated path.
        if !self.lane_state.is_empty() {
            let bytes = unsafe {
                std::slice::from_raw_parts_mut(
                    self.lane_state.as_mut_ptr().cast::<u8>(),
                    self.lane_state.len() * 8,
                )
            };
            for binding in self.lane_state_layout.iter().flatten() {
                let (offset, words) = (binding.offset, binding.words);
                let per_lane = (words * 8) / 64;
                let first = offset * 8 + per_lane * lane;
                bytes[first..first + per_lane].fill(0);
            }
        }
    }

    /// Clears every level and pending pulse bit of one lane.
    pub fn clear_lane_inputs(&mut self, circuit: &CompiledCircuit, lane: usize) {
        self.check_program(circuit);
        assert!(lane < LANES);
        let mask = !(1_u64 << lane);
        for word in &mut self.level_lanes {
            *word &= mask;
        }
        for word in &mut self.pulse_lanes {
            *word &= mask;
        }
    }

    /// Sets a persistent level input bit for one lane.
    pub fn set_level(&mut self, circuit: &CompiledCircuit, input: usize, lane: usize, value: bool) {
        self.check_program(circuit);
        assert!(lane < LANES);
        let mask = 1_u64 << lane;
        if value {
            self.level_lanes[input] |= mask;
        } else {
            self.level_lanes[input] &= !mask;
        }
    }

    /// Sets a persistent level input bit on **every** lane at once.
    ///
    /// For a signal that is the same for all of them by construction — a
    /// clock, a shared position, anything the caller broadcasts rather
    /// than gives each instance separately. Bit slicing already makes
    /// such a signal free to *evaluate*; this makes it free to *write*,
    /// where [`Self::set_level`] would cost 64 read-modify-writes per
    /// bit and per step.
    pub fn set_level_everywhere(&mut self, circuit: &CompiledCircuit, input: usize, value: bool) {
        self.check_program(circuit);
        self.level_lanes[input] = if value { !0 } else { 0 };
    }

    /// Asserts an input for tick 0 of the next step only (strobe).
    pub fn pulse(&mut self, circuit: &CompiledCircuit, input: usize, lane: usize) {
        self.check_program(circuit);
        assert!(lane < LANES);
        self.pulse_lanes[input] |= 1_u64 << lane;
    }

    /// Clears a not-yet-consumed pulse on one lane.
    pub fn clear_pulse(&mut self, circuit: &CompiledCircuit, input: usize, lane: usize) {
        self.check_program(circuit);
        assert!(lane < LANES);
        self.pulse_lanes[input] &= !(1_u64 << lane);
    }

    /// Runs one step (`ticks_per_step` ticks) for all 64 lanes.
    /// Outputs are readable afterwards via [`Self::output_word`].
    pub fn process_step(&mut self, circuit: &CompiledCircuit) {
        assert!(
            !self.private_state_is_live,
            "this CircuitState has run a JIT program that keeps a \
             region's state privately, so the register array no longer \
             holds all of it and the interpreter would carry on from \
             values that stopped being current. Use a fresh state, or a \
             program without private-state lowerings."
        );
        self.check_program(circuit);

        for tick in 0..circuit.ticks_per_step {
            for (index, binding) in circuit.inputs.iter().enumerate() {
                let mut word = self.level_lanes[index];
                if tick == 0 {
                    word |= self.pulse_lanes[index];
                }
                self.signals[binding.slot as usize] = word;
            }
            // Register q slots are the first `regs.len()` slots.
            self.signals[..self.reg_current.len()].copy_from_slice(&self.reg_current);

            // A bank's taps are register outputs, so they must be in
            // place before the instructions that read them. Its ingress
            // is sampled *after* the instruction pass, below, because
            // those same instructions may be what produces it — writing
            // the ingress first would overwrite the oldest ring entry
            // before the far tap had read it, silently shortening every
            // line by its own length.
            for (index, bank) in circuit.banks.iter().enumerate() {
                let head = self.heads[index];
                let ring = &self.rings[index];
                for &(distance, slot) in bank.taps.iter() {
                    let at = (head + bank.len - distance) % bank.len;
                    self.signals[slot as usize] = ring[at as usize];
                }
            }

            run_instrs(&mut self.signals, &circuit.instrs, &circuit.regions);

            // Simultaneous latch: collect, then commit.
            for (next, spec) in self.reg_next.iter_mut().zip(circuit.regs.iter()) {
                *next = self.signals[spec.d_slot as usize];
            }
            // The banks latch at the same moment, for the same reason:
            // the value entering a line this tick is whatever the
            // instructions have just produced, and the oldest entry has
            // already been read by the taps above.
            for (index, bank) in circuit.banks.iter().enumerate() {
                let head = self.heads[index];
                self.rings[index][head as usize] = self.signals[bank.ingress_slot as usize];
                self.heads[index] = (head + 1) % bank.len;
            }
            std::mem::swap(&mut self.reg_current, &mut self.reg_next);
        }
        self.pulse_lanes.fill(0);

        for (word, binding) in self.output_words.iter_mut().zip(circuit.outputs.iter()) {
            *word = self.signals[binding.slot as usize];
        }
    }

    /// Runs one step through the JIT backend. Semantics are identical
    /// to [`Self::process_step`]; the two backends are tested against
    /// each other.
    #[cfg(feature = "jit")]
    pub fn process_step_jit(&mut self, jit: &crate::jit::JitProgram) {
        // Whatever lane state this program keeps between ticks lives
        // here. Grown rather than reset: a program's private state is
        // meaningless to another program, and `check_program` already
        // refuses a state built for a different one.
        // A `CircuitState` is bound to the state ABI of the first
        // program it runs, not merely to the circuit.
        //
        // Two programs compiled from the same circuit -- one with
        // lowerings and one without -- share a `program_id` and are
        // otherwise interchangeable. They are not interchangeable when
        // one of them keeps a region's state privately: the register
        // array then holds that region's `init` forever, so switching to
        // the plain program restarts that region and switching back
        // resumes the old private state. Neither raises anything. It is
        // plausible output and it is wrong, which is the worst pair.
        let layout = jit.lane_state_regions();
        match &self.lane_state_layout {
            Some(bound) if bound.as_slice() != layout => panic!(
                "this CircuitState has run a program keeping {} region(s) of \
                 private state and is being given one keeping {}. They are \
                 not interchangeable: the register array holds only part of \
                 the truth for the first. Use a fresh state.",
                bound.len(),
                layout.len()
            ),
            Some(_) => {}
            None => self.lane_state_layout = Some(layout.to_vec()),
        }
        if jit.keeps_private_state() {
            self.private_state_is_live = true;
        }
        if self.lane_state.len() < jit.lane_state_words() {
            self.lane_state.resize(jit.lane_state_words(), 0);
        }
        assert_eq!(
            self.program_id, jit.program_id,
            "CircuitState used with a different JitProgram"
        );
        debug_assert_eq!(self.reg_current.len(), jit.reg_count);
        debug_assert_eq!(self.level_lanes.len(), jit.input_count);
        debug_assert_eq!(self.output_words.len(), jit.output_count);

        for tick in 0..jit.ticks_per_step {
            // Tick 0 sees level | pulse; later ticks see levels only.
            // The generated code reloads inputs each call, so folding
            // the pulses in temporarily costs nothing extra. Only the
            // bits the pulse actually *turned on* may be removed
            // afterwards — a pulse on an already-high level must not
            // clear it — so `pulse_lanes` is narrowed to those bits.
            if tick == 0 {
                for (level, pulse) in self.level_lanes.iter_mut().zip(self.pulse_lanes.iter_mut()) {
                    *pulse &= !*level;
                    *level |= *pulse;
                }
            } else if tick == 1 {
                for (level, pulse) in self.level_lanes.iter_mut().zip(self.pulse_lanes.iter()) {
                    *level &= !pulse;
                }
            }
            // SAFETY: buffer sizes are checked against the program
            // above and the regions are distinct allocations.
            unsafe {
                jit.tick(
                    self.reg_current.as_ptr(),
                    self.reg_next.as_mut_ptr(),
                    self.level_lanes.as_ptr(),
                    self.output_words.as_mut_ptr(),
                    self.lane_state.as_mut_ptr(),
                );
            }
            std::mem::swap(&mut self.reg_current, &mut self.reg_next);
        }
        // Single-tick steps still need the pulse bits removed.
        for (level, pulse) in self.level_lanes.iter_mut().zip(self.pulse_lanes.iter_mut()) {
            *level &= !*pulse;
            *pulse = 0;
        }
    }

    /// The lane bits of one output after the last processed step.
    pub fn output_word(&self, output: usize) -> u64 {
        self.output_words[output]
    }

    /// One lane's bit of one output after the last processed step.
    pub fn output_bit(&self, output: usize, lane: usize) -> bool {
        assert!(lane < LANES);
        (self.output_words[output] >> lane) & 1 == 1
    }
}

/// Runs one straight-line block of instructions over the signal array.
///
/// A region runs its fallback here, always. This evaluator has no
/// lowerings and wants none: it is the reference the others are checked
/// against, so it must compute a region the one way that cannot be
/// wrong -- by running the gates the region is made of.
///
/// The recursion is one level deep. Nested declarations are claimed by
/// the outermost at flattening time, so a fallback never contains a
/// region.
fn run_instrs(
    signals: &mut [u64],
    instrs: &[crate::compile::Instr],
    regions: &[crate::compile::CompiledRegion],
) {
    for instr in instrs {
        match *instr {
            crate::compile::Instr::Nand { a, b, out } => {
                signals[out as usize] = !(signals[a as usize] & signals[b as usize]);
            }
            crate::compile::Instr::FullAdder {
                a,
                b,
                cin,
                sum,
                cout,
            } => {
                let av = signals[a as usize];
                let bv = signals[b as usize];
                let cv = signals[cin as usize];
                let p = av ^ bv;
                signals[sum as usize] = p ^ cv;
                signals[cout as usize] = (av & bv) | (p & cv);
            }
            crate::compile::Instr::Xor { a, b, out } => {
                signals[out as usize] = signals[a as usize] ^ signals[b as usize];
            }
            crate::compile::Instr::And { a, b, out } => {
                signals[out as usize] = signals[a as usize] & signals[b as usize];
            }
            crate::compile::Instr::Not { a, out } => {
                signals[out as usize] = !signals[a as usize];
            }
            crate::compile::Instr::Region { region } => {
                run_instrs(signals, &regions[region as usize].fallback, regions);
            }
        }
    }
}

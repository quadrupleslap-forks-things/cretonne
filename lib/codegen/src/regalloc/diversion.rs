//! Register diversions.
//!
//! Normally, a value is assigned to a single register or stack location by the register allocator.
//! Sometimes, it is necessary to move register values to a different register in order to satisfy
//! instruction constraints.
//!
//! These register diversions are local to an EBB. No values can be diverted when entering a new
//! EBB.

use ir::{InstructionData, Opcode};
use ir::{StackSlot, Value, ValueLoc, ValueLocations};
use isa::{RegInfo, RegUnit};
use std::fmt;
use std::vec::Vec;

/// A diversion of a value from its original location to a new register or stack location.
///
/// In IR, a diversion is represented by a `regmove` instruction, possibly a chain of them for the
/// same value.
///
/// When tracking diversions, the `from` field is the original assigned value location, and `to` is
/// the current one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Diversion {
    /// The value that is diverted.
    pub value: Value,
    /// The original value location.
    pub from: ValueLoc,
    /// The current value location.
    pub to: ValueLoc,
}

impl Diversion {
    /// Make a new diversion.
    pub fn new(value: Value, from: ValueLoc, to: ValueLoc) -> Self {
        debug_assert!(from.is_assigned() && to.is_assigned());
        Self { value, from, to }
    }
}

/// Keep track of diversions in an EBB.
pub struct RegDiversions {
    current: Vec<Diversion>,
}

impl RegDiversions {
    /// Create a new empty diversion tracker.
    pub fn new() -> Self {
        Self { current: Vec::new() }
    }

    /// Clear the tracker, preparing for a new EBB.
    pub fn clear(&mut self) {
        self.current.clear()
    }

    /// Are there any diversions?
    pub fn is_empty(&self) -> bool {
        self.current.is_empty()
    }

    /// Get the current diversion of `value`, if any.
    pub fn diversion(&self, value: Value) -> Option<&Diversion> {
        self.current.iter().find(|d| d.value == value)
    }

    /// Get all current diversions.
    pub fn all(&self) -> &[Diversion] {
        self.current.as_slice()
    }

    /// Get the current location for `value`. Fall back to the assignment map for non-diverted
    /// values
    pub fn get(&self, value: Value, locations: &ValueLocations) -> ValueLoc {
        match self.diversion(value) {
            Some(d) => d.to,
            None => locations[value],
        }
    }

    /// Get the current register location for `value`, or panic if `value` isn't in a register.
    pub fn reg(&self, value: Value, locations: &ValueLocations) -> RegUnit {
        self.get(value, locations).unwrap_reg()
    }

    /// Get the current stack location for `value`, or panic if `value` isn't in a stack slot.
    pub fn stack(&self, value: Value, locations: &ValueLocations) -> StackSlot {
        self.get(value, locations).unwrap_stack()
    }

    /// Record any kind of move.
    ///
    /// The `from` location must match an existing `to` location, if any.
    pub fn divert(&mut self, value: Value, from: ValueLoc, to: ValueLoc) {
        debug_assert!(from.is_assigned() && to.is_assigned());
        if let Some(i) = self.current.iter().position(|d| d.value == value) {
            debug_assert_eq!(self.current[i].to, from, "Bad regmove chain for {}", value);
            if self.current[i].from != to {
                self.current[i].to = to;
            } else {
                self.current.swap_remove(i);
            }
        } else {
            self.current.push(Diversion::new(value, from, to));
        }
    }

    /// Record a register -> register move.
    pub fn regmove(&mut self, value: Value, from: RegUnit, to: RegUnit) {
        self.divert(value, ValueLoc::Reg(from), ValueLoc::Reg(to));
    }

    /// Record a register -> stack move.
    pub fn regspill(&mut self, value: Value, from: RegUnit, to: StackSlot) {
        self.divert(value, ValueLoc::Reg(from), ValueLoc::Stack(to));
    }

    /// Record a stack -> register move.
    pub fn regfill(&mut self, value: Value, from: StackSlot, to: RegUnit) {
        self.divert(value, ValueLoc::Stack(from), ValueLoc::Reg(to));
    }

    /// Apply the effect of `inst`.
    ///
    /// If `inst` is a `regmove`, `regfill`, or `regspill` instruction, update the diversions to
    /// match.
    pub fn apply(&mut self, inst: &InstructionData) {
        match *inst {
            InstructionData::RegMove {
                opcode: Opcode::Regmove,
                arg,
                src,
                dst,
            } => self.regmove(arg, src, dst),
            InstructionData::RegSpill {
                opcode: Opcode::Regspill,
                arg,
                src,
                dst,
            } => self.regspill(arg, src, dst),
            InstructionData::RegFill {
                opcode: Opcode::Regfill,
                arg,
                src,
                dst,
            } => self.regfill(arg, src, dst),
            _ => {}
        }
    }

    /// Drop any recorded move for `value`.
    ///
    /// Returns the `to` location of the removed diversion.
    pub fn remove(&mut self, value: Value) -> Option<ValueLoc> {
        self.current.iter().position(|d| d.value == value).map(
            |i| {
                self.current.swap_remove(i).to
            },
        )
    }

    /// Return an object that can display the diversions.
    pub fn display<'a, R: Into<Option<&'a RegInfo>>>(&'a self, regs: R) -> DisplayDiversions<'a> {
        DisplayDiversions(self, regs.into())
    }
}

/// Object that displays register diversions.
pub struct DisplayDiversions<'a>(&'a RegDiversions, Option<&'a RegInfo>);

impl<'a> fmt::Display for DisplayDiversions<'a> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{{")?;
        for div in self.0.all() {
            write!(
                f,
                " {}: {} -> {}",
                div.value,
                div.from.display(self.1),
                div.to.display(self.1)
            )?
        }
        write!(f, " }}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use entity::EntityRef;
    use ir::Value;

    #[test]
    fn inserts() {
        let mut divs = RegDiversions::new();
        let v1 = Value::new(1);
        let v2 = Value::new(2);

        divs.regmove(v1, 10, 12);
        assert_eq!(
            divs.diversion(v1),
            Some(&Diversion {
                value: v1,
                from: ValueLoc::Reg(10),
                to: ValueLoc::Reg(12),
            })
        );
        assert_eq!(divs.diversion(v2), None);

        divs.regmove(v1, 12, 11);
        assert_eq!(divs.diversion(v1).unwrap().to, ValueLoc::Reg(11));
        divs.regmove(v1, 11, 10);
        assert_eq!(divs.diversion(v1), None);
    }
}

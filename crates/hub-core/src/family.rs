//! The device families Hub knows, and what each takes over the link.
//!
//! **A device's feature list cannot say this.** `hello.features` names
//! groups (§6.1), and one group covers several requests: `runtime` is
//! status, push, launch, screenshot and logs alike. An M5Stack Tab5 lists
//! `runtime`, answers `runtime.push` and `runtime.screenshot` with
//! `unsupported`, sends no log line, and has no `runtime.native` (pocket-m5
//! `programs/m5stack-tab5/components/pocket_system/README.md`, "Pocket
//! Link"): its applets come from its own store page, which asks Hub
//! (`catalog` and `install`, §6.4), and its software is flashed over USB.
//!
//! So what Hub offers for a device is decided by its family, in this one
//! table. The control API carries the answer in each Device (`supports`), and
//! a client keeps no table of its own.

/// A Nintendo 3DS running Pocket Shell: the record's `kind`.
pub const KIND_3DS: &str = "3ds";
/// An M5Stack Tab5 running the pocket-m5 firmware.
pub const KIND_TAB5: &str = "tab5";

/// The family of a device that names `target`, the id of the host its
/// packages are built for (`3ds-dev`, `tab5-home`): the family's name alone
/// or followed by `-`. Empty for a target of no family Hub knows.
pub fn kind_of_target(target: &str) -> &'static str {
    [KIND_3DS, KIND_TAB5]
        .into_iter()
        .find(|kind| target.strip_prefix(kind).is_some_and(|rest| rest.is_empty() || rest.starts_with('-')))
        .unwrap_or("")
}

/// Something Hub asks of a device that not every family takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Operation {
    /// `runtime.push`: a `.pocket` sent from the computer.
    Push,
    /// `runtime.screenshot`.
    Screenshot,
    /// `runtime.logs` and the `log` events after it.
    Logs,
    /// `runtime.native.install`: Hub replaces the device's own program.
    Program,
}

impl Operation {
    pub const ALL: [Operation; 4] = [Self::Push, Self::Screenshot, Self::Logs, Self::Program];

    /// The name in a Device's `supports`.
    pub fn name(self) -> &'static str {
        match self {
            Self::Push => "push",
            Self::Screenshot => "screenshot",
            Self::Logs => "logs",
            Self::Program => "program",
        }
    }
}

/// Whether a device of `kind` takes `operation`.
///
/// | kind | push | screenshot | logs | program |
/// | --- | --- | --- | --- | --- |
/// | `3ds` | yes | yes | yes | yes |
/// | `tab5` | no | no | no | no |
/// | a family Hub does not know | yes | yes | yes | no |
///
/// A device of an unknown family is asked what its `runtime` group promises
/// (§6.1) and answers for itself; Hub holds no program for it.
pub fn supports(kind: &str, operation: Operation) -> bool {
    match kind {
        KIND_3DS => true,
        KIND_TAB5 => false,
        _ => operation != Operation::Program,
    }
}

/// The operations a device of `kind` takes, in the order of `Operation::ALL`.
pub fn supported(kind: &str) -> Vec<Operation> {
    Operation::ALL.into_iter().filter(|operation| supports(kind, *operation)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn families_by_target() {
        assert_eq!(kind_of_target("3ds-dev"), KIND_3DS);
        assert_eq!(kind_of_target("3ds"), KIND_3DS);
        assert_eq!(kind_of_target("tab5-home"), KIND_TAB5);
        assert_eq!(kind_of_target("tab5"), KIND_TAB5);
        // A name that only starts like a family's is another host.
        assert_eq!(kind_of_target("tab50"), "");
        assert_eq!(kind_of_target("3dsx-dev"), "");
        assert_eq!(kind_of_target("psp"), "");
        assert_eq!(kind_of_target(""), "");
    }

    #[test]
    fn what_each_family_takes() {
        assert_eq!(supported(KIND_3DS), Operation::ALL);
        assert!(supported(KIND_TAB5).is_empty());
        assert_eq!(supported(""), [Operation::Push, Operation::Screenshot, Operation::Logs]);
        assert_eq!(Operation::ALL.map(Operation::name), ["push", "screenshot", "logs", "program"]);
    }
}

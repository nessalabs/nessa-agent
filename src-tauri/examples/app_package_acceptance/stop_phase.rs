//! Stop state advanced by callbacks on the native event loop.

#[derive(Default, PartialEq, Eq)]
pub enum StopPhase {
    #[default]
    Live,
    Armed,
    Confirmed,
    Invalid,
}

impl StopPhase {
    pub fn arm(&mut self) -> bool {
        if *self != Self::Live {
            return false;
        }
        *self = Self::Armed;
        true
    }

    pub fn native_destroyed(&mut self) {
        *self = if *self == Self::Armed {
            Self::Confirmed
        } else {
            Self::Invalid
        };
    }

    pub fn invalidate(&mut self) {
        *self = Self::Invalid;
    }

    pub fn confirmed(&self) -> bool {
        *self == Self::Confirmed
    }
}

#[cfg(test)]
mod tests {
    use super::StopPhase;

    #[test]
    fn native_destroy_before_arming_refuses_stop() {
        let mut phase = StopPhase::default();
        phase.native_destroyed();
        assert!(!phase.arm());
        assert!(!phase.confirmed());
    }

    #[test]
    fn arming_only_confirms_after_native_destroy() {
        let mut phase = StopPhase::default();
        assert!(phase.arm());
        assert!(!phase.confirmed());
        phase.native_destroyed();
        assert!(phase.confirmed());
        assert!(!phase.arm());
    }

    #[test]
    fn ordinary_close_before_or_after_arming_invalidates_stop() {
        for armed in [false, true] {
            let mut phase = StopPhase::default();
            if armed {
                assert!(phase.arm());
            }
            phase.invalidate();
            assert!(!phase.arm());
            phase.native_destroyed();
            assert!(!phase.confirmed());
        }
    }
}

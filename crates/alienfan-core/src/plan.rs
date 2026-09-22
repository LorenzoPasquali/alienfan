//! Turning a preset into ordered sysfs writes (SPEC 6.4).

use crate::model::{Boost, Control, FanId, FanPair, Preset, Profile};

/// One sysfs write.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Write {
    Profile(Profile),
    /// `force` writes even if the node already reads `boost`: a profile
    /// change may reset the boost behind the driver's back (Phase 0, T4).
    Boost {
        fan: FanId,
        boost: Boost,
        force: bool,
    },
}

/// The hardware state a preset asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Target {
    pub profile: Profile,
    pub boost: FanPair<Boost>,
}

impl Target {
    /// `curve_boost` is the curve engine output, used only when the control
    /// is a curve. With `boost_requires_custom`, any control other than
    /// firmware runs on the `custom` profile.
    pub fn resolve(
        preset: &Preset,
        boost_requires_custom: bool,
        curve_boost: FanPair<Boost>,
    ) -> Self {
        let boost = match &preset.control {
            Control::Firmware => FanPair::both(Boost::MIN),
            Control::Fixed(boost) => *boost,
            Control::Curve(_) => curve_boost,
        };
        Self {
            profile: effective_profile(preset, boost_requires_custom),
            boost,
        }
    }
}

pub fn effective_profile(preset: &Preset, boost_requires_custom: bool) -> Profile {
    if boost_requires_custom && preset.control != Control::Firmware {
        Profile::Custom
    } else {
        preset.profile
    }
}

/// Profile first (only if it changes), then both boosts.
pub fn plan(target: &Target, current_profile: Profile) -> Vec<Write> {
    let profile_changes = target.profile != current_profile;
    let profile = profile_changes.then_some(Write::Profile(target.profile));
    let boosts = FanId::ALL.iter().map(|&fan| Write::Boost {
        fan,
        boost: target.boost[fan],
        force: profile_changes,
    });
    profile.into_iter().chain(boosts).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn preset(profile: Profile, control: Control) -> Preset {
        Preset { profile, control }
    }

    #[test]
    fn profile_is_written_before_boost_and_forces_it() {
        let target = Target::resolve(
            &preset(
                Profile::Quiet,
                Control::Fixed(FanPair::new(Boost(10), Boost(20))),
            ),
            false,
            FanPair::default(),
        );
        assert_eq!(
            plan(&target, Profile::Balanced),
            [
                Write::Profile(Profile::Quiet),
                Write::Boost {
                    fan: FanId::Cpu,
                    boost: Boost(10),
                    force: true
                },
                Write::Boost {
                    fan: FanId::Gpu,
                    boost: Boost(20),
                    force: true
                },
            ]
        );
    }

    #[test]
    fn same_profile_is_not_written() {
        let target = Target::resolve(
            &preset(Profile::Quiet, Control::Firmware),
            false,
            FanPair::default(),
        );
        assert_eq!(
            plan(&target, Profile::Quiet),
            [
                Write::Boost {
                    fan: FanId::Cpu,
                    boost: Boost(0),
                    force: false
                },
                Write::Boost {
                    fan: FanId::Gpu,
                    boost: Boost(0),
                    force: false
                },
            ]
        );
    }

    #[test]
    fn boost_requires_custom_overrides_the_profile() {
        let fixed = preset(Profile::Quiet, Control::Fixed(FanPair::both(Boost(50))));
        assert_eq!(
            Target::resolve(&fixed, true, FanPair::default()).profile,
            Profile::Custom
        );
        assert_eq!(
            Target::resolve(&fixed, false, FanPair::default()).profile,
            Profile::Quiet
        );

        let curve = preset(Profile::Quiet, Control::Curve("x".into()));
        let t = Target::resolve(&curve, true, FanPair::both(Boost(7)));
        assert_eq!(t.profile, Profile::Custom);
        assert_eq!(t.boost, FanPair::both(Boost(7)));

        let firmware = preset(Profile::Quiet, Control::Firmware);
        assert_eq!(
            Target::resolve(&firmware, true, FanPair::default()).profile,
            Profile::Quiet
        );
    }

    #[test]
    fn boot_curve_keeps_the_preset_profile() {
        let curve = preset(Profile::Quiet, Control::Curve("x".into()));
        let t = Target::resolve(&curve.for_boot(), true, FanPair::both(Boost(99)));
        assert_eq!(t.profile, Profile::Quiet);
        assert_eq!(t.boost, FanPair::both(Boost::MIN));
    }
}

//! Direct environment reads observed during widget callbacks, independent of cache policy.

bitflags::bitflags! {
    #[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
    pub(super) struct EnvReads: u8 {
        const DPI = 1 << 0;
        const VIEWPORT_SIZE = 1 << 1;
        const FRAME_TIME = 1 << 2;
    }
}

#[derive(Clone, Copy)]
pub(super) enum ReadPhase {
    Build,
    Update,
    Measure,
    Layout,
    Render,
}

/// Latest normally returned callback's direct reads, not a cache validity record.
/// None means unobserved; Some(empty) means the callback read no tracked environment values.
/// Build/update observations do not track the lifetime of values stored inside the widget.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(super) struct NodeEnvReads {
    build: Option<EnvReads>,
    update: Option<EnvReads>,
    measure: Option<EnvReads>,
    layout: Option<EnvReads>,
    render: Option<EnvReads>,
}

impl NodeEnvReads {
    pub(super) fn record(&mut self, phase: ReadPhase, reads: EnvReads) {
        let slot = match phase {
            ReadPhase::Build => &mut self.build,
            ReadPhase::Update => &mut self.update,
            ReadPhase::Measure => &mut self.measure,
            ReadPhase::Layout => &mut self.layout,
            ReadPhase::Render => &mut self.render,
        };
        *slot = Some(reads);
    }

    #[cfg(test)]
    pub(super) fn get(&self, phase: ReadPhase) -> Option<EnvReads> {
        match phase {
            ReadPhase::Build => self.build,
            ReadPhase::Update => self.update,
            ReadPhase::Measure => self.measure,
            ReadPhase::Layout => self.layout,
            ReadPhase::Render => self.render,
        }
    }
}

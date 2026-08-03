use crate::types::grow_size::GrowSize;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlexWrap {
    NoWrap,
    Wrap,
    WrapReverse,
}

/// control the alignment of children on the **main axis**
#[derive(Clone, PartialEq)]
pub enum JustifyContent {
    FlexStart { gap: GrowSize },
    FlexEnd { gap: GrowSize },
    Center { gap: GrowSize },
    SpaceBetween,
    SpaceAround,
    SpaceEvenly,
}

/// control the alignment of children on the **cross axis**
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlignItems {
    Start,
    End,
    Center,
}

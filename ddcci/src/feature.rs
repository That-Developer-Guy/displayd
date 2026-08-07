#[derive(Debug, Copy, Clone, Eq, PartialEq, Hash)]
pub enum Feature {
    Brightness,
    Contrast,
    InputSource,
    Volume,
    Mute,
    PowerMode,

    Raw(u8),
}

impl Feature {
    pub fn code(self) -> u8 {
        match self {
            Feature::Brightness => 0x10,
            Feature::Contrast => 0x12,
            Feature::InputSource => 0x60,
            Feature::Volume => 0x62,
            Feature::Mute => 0x8d,
            Feature::PowerMode => 0xd6,
            Feature::Raw(code) => code,
        }
    }
}

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

impl From<Feature> for u8 {
    fn from(feature: Feature) -> Self {
        match feature {
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

impl From<u8> for Feature {
    fn from(code: u8) -> Self {
        match code {
            0x10 => Feature::Brightness,
            0x12 => Feature::Contrast,
            0x60 => Feature::InputSource,
            0x62 => Feature::Volume,
            0x8d => Feature::Mute,
            0xd6 => Feature::PowerMode,
            other => Feature::Raw(other),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Transition {
    LR,
    LW,
    FR,
    FW,
}

impl Transition {
    pub fn as_bytes(&self) -> &'static [u8] {
        match self {
            Transition::LR => b"LR",
            Transition::LW => b"LW",
            Transition::FR => b"FR",
            Transition::FW => b"FW",
        }
    }
}

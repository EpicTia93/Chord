use serde::{Deserialize, Serialize};
use typeshare::typeshare;

#[typeshare]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ChordLayout {
    #[default]
    SidebarLeft,
    FloatingBottom,
}

impl ChordLayout {
    pub fn next(self) -> Self {
        match self {
            Self::SidebarLeft => Self::FloatingBottom,
            Self::FloatingBottom => Self::SidebarLeft,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saved_layouts_round_trip_and_cycle_back_to_the_default() {
        let sidebar = ChordLayout::default();
        assert_eq!(sidebar, ChordLayout::SidebarLeft);
        assert_eq!(sidebar.next(), ChordLayout::FloatingBottom);
        assert_eq!(sidebar.next().next(), sidebar);
        for (value, layout) in [
            ("sidebar-left", sidebar),
            ("floating-bottom", sidebar.next()),
        ] {
            let saved = serde_json::to_value(layout).unwrap();
            assert_eq!(saved, value);
            assert_eq!(
                serde_json::from_value::<ChordLayout>(saved).unwrap(),
                layout
            );
        }
    }
}

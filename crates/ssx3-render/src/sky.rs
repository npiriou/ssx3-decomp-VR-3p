//! Native polar dome plus four paired mountain/upper-sky sectors.
//! Visibility removes draws, not their identity or place in this layout.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Layout {
    pub base: u16,
}

impl Layout {
    pub fn textures(self) -> [u16; 9] {
        [0, 8, 7, 6, 1, 5, 4, 3, 2].map(|offset| self.base + offset)
    }

    pub fn detect(textures: &[Option<u16>]) -> Option<Self> {
        let base = textures.first().copied().flatten()?;
        if textures.len() < 2 || base > 1991 {
            return None;
        }
        let layout = Self { base };
        let order = layout.textures();
        let mut next = 0;
        for texture in textures {
            let offset = order[next..].iter().position(|id| Some(*id) == *texture)?;
            next += offset + 1;
        }
        Some(layout)
    }

    pub fn sector(self, texture: u16) -> Option<(usize, bool)> {
        Some(match texture.checked_sub(self.base)? {
            // Artwork slots pair upper/lower textures. Meshes determine
            // their spatial order; it varies between native sky assets.
            8 => (0, false),
            7 => (1, false),
            6 => (2, false),
            1 => (3, false),
            5 => (0, true),
            4 => (1, true),
            3 => (2, true),
            2 => (3, true),
            _ => return None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn culled_panels_keep_the_layout_on_every_native_palette() {
        for base in [281, 420, 508, 630, 766, 900] {
            let layout = Layout { base };
            let ids = layout.textures();
            for mask in 1u16..256 {
                let mut visible = vec![Some(base)];
                visible.extend(
                    (0..8)
                        .filter(|i| mask & (1 << i) != 0)
                        .map(|i| Some(ids[i + 1])),
                );
                assert_eq!(Layout::detect(&visible), Some(layout));
            }
        }
        assert!(Layout::detect(&[Some(281), Some(283), Some(289)]).is_none());
        assert!(Layout::detect(&[Some(281), None]).is_none());
    }

    #[test]
    fn upper_and_lower_artwork_slots_are_paired() {
        let layout = Layout { base: 766 };
        for (lower, upper) in [(774, 771), (773, 770), (772, 769), (767, 768)] {
            let (a, a_upper) = layout.sector(lower).unwrap();
            let (b, b_upper) = layout.sector(upper).unwrap();
            assert_eq!(a, b);
            assert!(!a_upper && b_upper);
        }
    }
}

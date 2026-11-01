use crate::selection::Selection;
use eframe::egui::{Modifiers, Pos2, Rect, Vec2};
#[derive(Clone, Copy)]
pub struct RowHit {
    pub index: usize,
    pub rect: Rect,
    pub name: Rect,
}
pub struct Pointer {
    pub pos: Option<Pos2>,
    pub pressed: bool,
    pub held: bool,
    pub released: bool,
    pub escape: bool,
    pub modifiers: Modifiers,
}
enum Phase {
    Press {
        origin: Pos2,
        row: Option<usize>,
        on_name: bool,
        base: Selection,
        modifiers: Modifiers,
    },
    Box {
        origin: Pos2,
        base: Selection,
        modifiers: Modifiers,
    },
}
#[derive(Default)]
pub struct ListDrag {
    phase: Option<Phase>,
    pub rectangle: Option<Rect>,
}
pub struct Layout<'a> {
    pub viewport: Rect,
    pub offset: Vec2,
    pub row_count: usize,
    pub row_height: f32,
    pub spacing: f32,
    pub hits: &'a [RowHit],
}
pub struct Update {
    pub files: Option<usize>,
    pub auto_scroll: f32,
}
impl ListDrag {
    pub fn active(&self) -> bool {
        self.phase.is_some()
    }
    pub fn cancel(&mut self) {
        self.phase = None;
        self.rectangle = None;
    }
    pub fn update(
        &mut self,
        input: Pointer,
        layout: Layout<'_>,
        selection: &mut Selection,
    ) -> Update {
        let Layout {
            viewport,
            offset,
            row_count,
            row_height,
            spacing,
            hits,
        } = layout;
        let stride = row_height + spacing;
        let mut output = Update {
            files: None,
            auto_scroll: 0.,
        };
        self.rectangle = None;
        if input.pressed
            && let Some(pos) = input.pos.filter(|p| viewport.contains(*p))
        {
            let hit = hits.iter().find(|h| h.rect.contains(pos));
            self.phase = Some(Phase::Press {
                origin: pos,
                row: hit.map(|h| h.index),
                on_name: hit.is_some_and(|h| h.name.contains(pos)),
                base: selection.clone(),
                modifiers: input.modifiers,
            });
        }
        let Some(phase) = self.phase.take() else {
            return output;
        };
        let mut phase = match phase {
            Phase::Press {
                origin,
                row,
                on_name,
                base,
                modifiers,
            } => {
                if input.escape {
                    *selection = base;
                    return output;
                }
                if let Some(pos) = input.pos
                    && origin.distance(pos) > 6.
                    && (input.held || input.released)
                {
                    if on_name {
                        if input.held {
                            output.files = row;
                        }
                        return output;
                    }
                    Phase::Box {
                        origin: origin - viewport.min.to_vec2() + offset,
                        base,
                        modifiers,
                    }
                } else {
                    if input.released {
                        if row.is_none() && !modifiers.ctrl && !modifiers.shift {
                            selection.clear();
                        }
                        return output;
                    }
                    self.phase = Some(Phase::Press {
                        origin,
                        row,
                        on_name,
                        base,
                        modifiers,
                    });
                    return output;
                }
            }
            phase => phase,
        };
        if let Phase::Box {
            origin,
            base,
            modifiers,
        } = &mut phase
        {
            if input.escape {
                *selection = base.clone();
                return output;
            }
            if let Some(pos) = input.pos {
                let current = pos - viewport.min.to_vec2() + offset;
                let min = origin.y.min(current.y).max(0.);
                let max = origin.y.max(current.y);
                let mut items = if modifiers.ctrl || modifiers.shift {
                    base.items.clone()
                } else {
                    Default::default()
                };
                let mut last = None;
                if max >= 0. && row_count > 0 {
                    let first = (min / stride) as usize;
                    let end = ((max / stride) as usize).min(row_count - 1);
                    for n in first..=end {
                        if n as f32 * stride + row_height < min {
                            continue;
                        }
                        if modifiers.ctrl && !modifiers.shift {
                            if !items.remove(&n) {
                                items.insert(n);
                            }
                        } else {
                            items.insert(n);
                        }
                        last = Some(n);
                    }
                }
                selection.replace(items, last.or(base.focus));
                self.rectangle = Some(Rect::from_two_pos(
                    *origin + viewport.min.to_vec2() - offset,
                    pos,
                ));
                if input.held {
                    if pos.y < viewport.top() + 18. {
                        output.auto_scroll = -1.;
                    } else if pos.y > viewport.bottom() - 18. {
                        output.auto_scroll = 1.;
                    }
                }
            }
        }
        if input.held {
            self.phase = Some(phase);
        } else {
            self.rectangle = None;
        }
        output
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui;
    #[test]
    fn rubber_band_covers_virtual_rows_and_modifiers_restore_on_escape() {
        let viewport = Rect::from_min_size(Pos2::ZERO, egui::vec2(600., 200.));
        let mut drag = ListDrag::default();
        let mut s = Selection::default();
        s.single(1);
        let ptr = |pos, pressed, held, released, escape| Pointer {
            pos: Some(pos),
            pressed,
            held,
            released,
            escape,
            modifiers: Modifiers::NONE,
        };
        drag.update(
            ptr(egui::pos2(500., 190.), true, true, false, false),
            Layout {
                viewport,
                offset: Vec2::ZERO,
                row_count: 5000,
                row_height: 27.,
                spacing: 9.,
                hits: &[],
            },
            &mut s,
        );
        drag.update(
            ptr(egui::pos2(400., 10.), false, true, false, false),
            Layout {
                viewport,
                offset: Vec2::ZERO,
                row_count: 5000,
                row_height: 27.,
                spacing: 9.,
                hits: &[],
            },
            &mut s,
        );
        assert_eq!(
            s.items.iter().copied().collect::<Vec<_>>(),
            vec![0, 1, 2, 3, 4, 5]
        );
        drag.update(
            ptr(egui::pos2(400., 10.), false, true, false, true),
            Layout {
                viewport,
                offset: Vec2::ZERO,
                row_count: 5000,
                row_height: 27.,
                spacing: 9.,
                hits: &[],
            },
            &mut s,
        );
        assert_eq!(s.items, std::collections::BTreeSet::from([1]));
    }
}

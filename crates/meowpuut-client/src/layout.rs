#![forbid(unsafe_code)]
//! Candidate bar layout (`CollapsedCandidateLayout`, `expanded_candidate_rows`),
//! macOS screen-clamped window framing (`candidate_frame`), and iOS keyboard
//! geometry (`KeyboardLayoutMetrics`).
//!
//! # Examples
//!
//! ```
//! use meowpuut_client::layout::{
//!     candidate_frame, expanded_candidate_rows, CollapsedCandidateLayout, KeyboardLayoutMetrics,
//!     Rect,
//! };
//!
//! let cands = vec!["你好".into(), "你".into(), "好".into(), "你們".into(), "世界和平".into()];
//! let collapsed = CollapsedCandidateLayout::compute(&cands, 320.0);
//! assert!(!collapsed.visible_indices.is_empty());
//!
//! let rows = expanded_candidate_rows(&cands, 4);
//! assert!(!rows.is_empty());
//!
//! let caret = Rect { x: 100.0, y: 20.0, width: 2.0, height: 18.0 };
//! let screen = Rect { x: 0.0, y: 0.0, width: 1440.0, height: 900.0 };
//! let frame = candidate_frame(caret, (260.0, 44.0), screen);
//! assert!(frame.y >= screen.y);
//!
//! let metrics = KeyboardLayoutMetrics::for_viewport(390.0, 844.0, false);
//! assert!(metrics.key_height >= 40.0);
//! ```

/// 2D rectangle used for macOS floating candidate panel positioning and iOS key frames.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// Estimates the visual pill width in points for a candidate string in the collapsed bar.
pub fn estimate_pill_width(text: &str) -> f64 {
    let mut units = 0.0f64;
    for ch in text.chars() {
        if ch.is_ascii() {
            units += 9.5;
        } else {
            units += 17.0;
        }
    }
    (units + 24.0).clamp(42.0, 220.0)
}

/// Horizontal collapsed candidate strip layout that reserves space for the expand chevron
/// so candidate pills never overlap the trailing expand button.
#[derive(Debug, Clone, PartialEq)]
pub struct CollapsedCandidateLayout {
    pub visible_indices: Vec<usize>,
    pub pill_widths: Vec<f64>,
    pub can_expand: bool,
    pub reserved_expand_width: f64,
}

impl CollapsedCandidateLayout {
    /// Computes which candidate indices fit inside `bar_width` without overlapping the expand button.
    pub fn compute(candidates: &[String], bar_width: f64) -> Self {
        if candidates.is_empty() || bar_width <= 0.0 {
            return Self {
                visible_indices: Vec::new(),
                pill_widths: Vec::new(),
                can_expand: false,
                reserved_expand_width: 0.0,
            };
        }
        let expand_button_width = 40.0;
        let horizontal_padding = 12.0;
        let spacing = 6.0;
        let usable = (bar_width - expand_button_width - horizontal_padding).max(48.0);

        let mut visible_indices = Vec::new();
        let mut pill_widths = Vec::new();
        let mut used = 0.0f64;

        for (idx, text) in candidates.iter().enumerate() {
            let w = estimate_pill_width(text);
            let gap = if visible_indices.is_empty() { 0.0 } else { spacing };
            if !visible_indices.is_empty() && used + gap + w > usable {
                break;
            }
            used += gap + w.min(usable);
            visible_indices.push(idx);
            pill_widths.push(w.min(usable));
        }

        let can_expand = candidates.len() > visible_indices.len() || candidates.len() > 4;
        Self {
            visible_indices,
            pill_widths,
            can_expand,
            reserved_expand_width: if can_expand { expand_button_width } else { 0.0 },
        }
    }
}

/// Cell in an expanded multi-column candidate grid row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CandidateGridCell {
    pub index: usize,
    pub text: String,
    pub span: usize,
}

/// Partitions `candidates` into rows of `max_columns` (typically `4`), assigning wider
/// column spans (`1`, `2`, or `max_columns`) to longer phrases.
pub fn expanded_candidate_rows(
    candidates: &[String],
    max_columns: usize,
) -> Vec<Vec<CandidateGridCell>> {
    let cols = max_columns.max(1);
    let mut rows: Vec<Vec<CandidateGridCell>> = Vec::new();
    let mut current_row: Vec<CandidateGridCell> = Vec::new();
    let mut used_cols = 0usize;

    for (index, text) in candidates.iter().enumerate() {
        let char_count = text.chars().count();
        let span = if char_count <= 2 {
            1
        } else if char_count <= 5 {
            2.min(cols)
        } else {
            cols
        };
        if !current_row.is_empty() && used_cols + span > cols {
            rows.push(std::mem::take(&mut current_row));
            used_cols = 0;
        }
        current_row.push(CandidateGridCell {
            index,
            text: text.clone(),
            span,
        });
        used_cols += span;
        if used_cols >= cols {
            rows.push(std::mem::take(&mut current_row));
            used_cols = 0;
        }
    }
    if !current_row.is_empty() {
        rows.push(current_row);
    }
    rows
}

/// Computes the screen-clamped floating candidate panel frame on macOS (`AppKit` coordinate space
/// where `y` increases upward). Flips the panel above the caret if placing it below would clip.
pub fn candidate_frame(caret: Rect, panel_size: (f64, f64), visible_screen: Rect) -> Rect {
    let width = panel_size.0.min(visible_screen.width);
    let height = panel_size.1.min(visible_screen.height);
    let gap = 6.0;

    let mut x = caret.x;
    let max_x = visible_screen.x + visible_screen.width - width;
    if x > max_x {
        x = max_x;
    }
    if x < visible_screen.x {
        x = visible_screen.x;
    }

    let below_y = caret.y - height - gap;
    let y = if below_y >= visible_screen.y {
        below_y
    } else {
        (caret.y + caret.height + gap).min(visible_screen.y + visible_screen.height - height)
    };

    Rect {
        x,
        y,
        width,
        height,
    }
}

/// Responsive geometry metrics for the iOS keyboard extension across portrait, landscape,
/// and floating/iPad viewports.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct KeyboardLayoutMetrics {
    pub candidate_bar_height: f64,
    pub key_height: f64,
    pub row_spacing: f64,
    pub key_spacing: f64,
    pub horizontal_inset: f64,
    pub total_height: f64,
}

impl KeyboardLayoutMetrics {
    /// Computes keyboard metrics for the given viewport width, height, and orientation.
    pub fn for_viewport(width: f64, height: f64, is_landscape: bool) -> Self {
        let candidate_bar_height = if is_landscape { 38.0 } else { 44.0 };
        let key_height = if is_landscape {
            34.0
        } else if width >= 700.0 {
            52.0
        } else if height < 680.0 {
            40.0
        } else {
            44.0
        };
        let row_spacing = if is_landscape { 6.0 } else { 10.0 };
        let key_spacing = if width >= 700.0 { 8.0 } else { 6.0 };
        let horizontal_inset = if width >= 700.0 { 10.0 } else { 4.0 };
        let total_height = candidate_bar_height + 4.0 * key_height + 4.0 * row_spacing + 8.0;
        Self {
            candidate_bar_height,
            key_height,
            row_spacing,
            key_spacing,
            horizontal_inset,
            total_height,
        }
    }
}

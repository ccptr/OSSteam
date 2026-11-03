// SPDX-License-Identifier: GPL-2.0-or-later
//! Element styles (ADR 0004, "Layout"): how an element is laid out
//! (flexbox, through `taffy`) and what it paints. Lengths are logical
//! pixels; the toolkit scales them to device pixels when it lays out.
//!
//! An element is a flex container that stacks its children in a column
//! unless told otherwise. Text properties (colour, size, weight, line
//! height, wrapping) are inherited by the text inside an element.

/// The pointer shapes [`Styled::cursor`] takes.
pub use winit::window::CursorIcon;

use crate::scene::Color;

/// A length in logical pixels, or a fraction of the containing block.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Length {
    /// From the content, or from the flex layout.
    #[default]
    Auto,
    Px(f32),
    /// 1.0 is the whole of the containing block.
    Fraction(f32),
}

impl From<f32> for Length {
    fn from(v: f32) -> Self {
        Length::Px(v)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Axis {
    Row,
    #[default]
    Column,
}

/// Where children sit across the main axis, or where one child sits.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Align {
    Start,
    Center,
    End,
    /// Fill the cross axis.
    #[default]
    Stretch,
}

/// Where children sit along the main axis.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Justify {
    #[default]
    Start,
    Center,
    End,
    /// The first at the start, the last at the end, the space shared
    /// between them.
    Between,
}

/// What happens to content larger than the element.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Overflow {
    #[default]
    Visible,
    Clip,
}

/// Top, right, bottom, left.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Edges<T> {
    pub top: T,
    pub right: T,
    pub bottom: T,
    pub left: T,
}

impl<T: Copy> Edges<T> {
    pub const fn all(v: T) -> Self {
        Self {
            top: v,
            right: v,
            bottom: v,
            left: v,
        }
    }

    pub const fn xy(x: T, y: T) -> Self {
        Self {
            top: y,
            right: x,
            bottom: y,
            left: x,
        }
    }
}

/// A drop shadow: the element's rounded rectangle, grown by `spread`,
/// moved by the offset and blurred with a Gaussian of standard deviation
/// `blur`, drawn under the element.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Shadow {
    pub color: Color,
    pub offset_x: f32,
    pub offset_y: f32,
    pub blur: f32,
    pub spread: f32,
}

/// Inherited text properties; `None` keeps the parent's.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TextStyle {
    pub color: Option<Color>,
    pub size: Option<f32>,
    pub weight: Option<u16>,
    /// Logical pixels.
    pub line_height: Option<f32>,
    pub nowrap: Option<bool>,
}

/// The text properties in force, after inheritance.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ResolvedText {
    pub color: Color,
    pub size: f32,
    pub weight: u16,
    /// `None`: [`ResolvedText::LINE_SPACING`] times the size.
    pub line_height: Option<f32>,
    pub nowrap: bool,
}

impl Default for ResolvedText {
    fn default() -> Self {
        Self {
            color: Color::rgb(0x000000),
            size: 14.0,
            weight: 400,
            line_height: None,
            nowrap: false,
        }
    }
}

impl ResolvedText {
    /// The line height relative to the font size, unless an element sets one.
    pub const LINE_SPACING: f32 = 1.4;

    pub fn inherit(mut self, s: &TextStyle) -> Self {
        if let Some(c) = s.color {
            self.color = c;
        }
        if let Some(v) = s.size {
            self.size = v;
            // A line height set for another size does not carry over.
            self.line_height = None;
        }
        if let Some(w) = s.weight {
            self.weight = w;
        }
        if s.line_height.is_some() {
            self.line_height = s.line_height;
        }
        if let Some(n) = s.nowrap {
            self.nowrap = n;
        }
        self
    }

    /// The line height in whole logical pixels.
    pub fn line_height(&self) -> f32 {
        self.line_height
            .unwrap_or(self.size * Self::LINE_SPACING)
            .round()
    }
}

/// Everything about how an element is laid out and painted.
#[derive(Clone, Debug, PartialEq)]
pub struct Style {
    /// Not laid out or painted at all.
    pub hidden: bool,
    pub axis: Axis,
    pub wrap: bool,
    pub grow: f32,
    pub shrink: f32,
    pub basis: Length,
    pub align_items: Align,
    pub align_self: Option<Align>,
    pub justify: Justify,
    pub gap: f32,
    pub width: Length,
    pub height: Length,
    pub min_width: Length,
    pub min_height: Length,
    pub max_width: Length,
    pub max_height: Length,
    pub padding: Edges<f32>,
    pub margin: Edges<f32>,
    pub border_widths: Edges<f32>,
    /// Taken out of the flow and placed by `inset` in its parent.
    pub absolute: bool,
    pub inset: Edges<Length>,
    pub overflow: Overflow,

    pub background: Option<Color>,
    pub border_color: Option<Color>,
    pub radius: f32,
    pub shadows: Vec<Shadow>,
    /// Multiplies the alpha of the element and everything in it.
    pub opacity: Option<f32>,
    pub cursor: Option<CursorIcon>,
    pub text: TextStyle,
}

impl Default for Style {
    fn default() -> Self {
        Self {
            hidden: false,
            axis: Axis::Column,
            wrap: false,
            grow: 0.0,
            shrink: 1.0,
            basis: Length::Auto,
            align_items: Align::Stretch,
            align_self: None,
            justify: Justify::Start,
            gap: 0.0,
            width: Length::Auto,
            height: Length::Auto,
            min_width: Length::Auto,
            min_height: Length::Auto,
            max_width: Length::Auto,
            max_height: Length::Auto,
            padding: Edges::default(),
            margin: Edges::default(),
            border_widths: Edges::default(),
            absolute: false,
            inset: Edges::all(Length::Auto),
            overflow: Overflow::Visible,
            background: None,
            border_color: None,
            radius: 0.0,
            shadows: Vec::new(),
            opacity: None,
            cursor: None,
            text: TextStyle::default(),
        }
    }
}

impl Style {
    /// Lays `other`'s paint properties over these: what a hover or focus
    /// style may change. Neither can change the layout.
    pub fn refine_paint(&mut self, other: &Style) {
        if other.background.is_some() {
            self.background = other.background;
        }
        if other.border_color.is_some() {
            self.border_color = other.border_color;
        }
        if other.opacity.is_some() {
            self.opacity = other.opacity;
        }
        if other.text.color.is_some() {
            self.text.color = other.text.color;
        }
        if !other.shadows.is_empty() {
            self.shadows.clone_from(&other.shadows);
        }
    }
}

/// The style builder methods, for anything with a [`Style`].
pub trait Styled: Sized {
    fn style_mut(&mut self) -> &mut Style;

    /// Lays children out left to right.
    fn row(mut self) -> Self {
        self.style_mut().axis = Axis::Row;
        self
    }
    /// Lays children out top to bottom (the default).
    fn column(mut self) -> Self {
        self.style_mut().axis = Axis::Column;
        self
    }
    /// Starts a new line of children when one is full.
    fn wrap(mut self) -> Self {
        self.style_mut().wrap = true;
        self
    }
    fn hidden(mut self, hidden: bool) -> Self {
        self.style_mut().hidden = hidden;
        self
    }
    /// Takes the space its parent has left over, shared with the other
    /// elements that grow; its own content does not count.
    fn grow(mut self) -> Self {
        let s = self.style_mut();
        s.grow = 1.0;
        s.basis = Length::Px(0.0);
        self
    }
    /// Keeps its size when its parent runs out of room.
    fn no_shrink(mut self) -> Self {
        self.style_mut().shrink = 0.0;
        self
    }
    fn align(mut self, a: Align) -> Self {
        self.style_mut().align_items = a;
        self
    }
    fn align_self(mut self, a: Align) -> Self {
        self.style_mut().align_self = Some(a);
        self
    }
    fn justify(mut self, j: Justify) -> Self {
        self.style_mut().justify = j;
        self
    }
    /// Children centred on both axes.
    fn center(self) -> Self {
        self.align(Align::Center).justify(Justify::Center)
    }
    fn gap(mut self, v: f32) -> Self {
        self.style_mut().gap = v;
        self
    }

    fn width(mut self, v: impl Into<Length>) -> Self {
        self.style_mut().width = v.into();
        self
    }
    fn height(mut self, v: impl Into<Length>) -> Self {
        self.style_mut().height = v.into();
        self
    }
    fn size(self, w: impl Into<Length>, h: impl Into<Length>) -> Self {
        self.width(w).height(h)
    }
    fn fill_width(self) -> Self {
        self.width(Length::Fraction(1.0))
    }
    fn fill_height(self) -> Self {
        self.height(Length::Fraction(1.0))
    }
    /// The whole of the containing block.
    fn fill(self) -> Self {
        self.fill_width().fill_height()
    }
    fn min_width(mut self, v: impl Into<Length>) -> Self {
        self.style_mut().min_width = v.into();
        self
    }
    fn min_height(mut self, v: impl Into<Length>) -> Self {
        self.style_mut().min_height = v.into();
        self
    }
    fn max_width(mut self, v: impl Into<Length>) -> Self {
        self.style_mut().max_width = v.into();
        self
    }
    fn max_height(mut self, v: impl Into<Length>) -> Self {
        self.style_mut().max_height = v.into();
        self
    }

    fn padding(mut self, p: Edges<f32>) -> Self {
        self.style_mut().padding = p;
        self
    }
    fn pad(self, v: f32) -> Self {
        self.padding(Edges::all(v))
    }
    fn pad_xy(self, x: f32, y: f32) -> Self {
        self.padding(Edges::xy(x, y))
    }
    fn margin(mut self, m: Edges<f32>) -> Self {
        self.style_mut().margin = m;
        self
    }

    /// Taken out of the flow, placed by [`Styled::inset`] inside its
    /// parent's padding box.
    fn absolute(mut self) -> Self {
        self.style_mut().absolute = true;
        self
    }
    fn inset(mut self, i: Edges<Length>) -> Self {
        self.style_mut().inset = i;
        self
    }
    /// Clips what is drawn inside to the element's bounds.
    fn clip(mut self) -> Self {
        self.style_mut().overflow = Overflow::Clip;
        self
    }

    fn background(mut self, c: Color) -> Self {
        self.style_mut().background = Some(c);
        self
    }
    fn border(mut self, width: f32, color: Color) -> Self {
        let s = self.style_mut();
        s.border_widths = Edges::all(width);
        s.border_color = Some(color);
        self
    }
    /// A border on some sides only.
    fn border_sides(mut self, widths: Edges<f32>, color: Color) -> Self {
        let s = self.style_mut();
        s.border_widths = widths;
        s.border_color = Some(color);
        self
    }
    fn border_color(mut self, c: Color) -> Self {
        self.style_mut().border_color = Some(c);
        self
    }
    fn radius(mut self, r: f32) -> Self {
        self.style_mut().radius = r;
        self
    }
    fn shadow(mut self, s: Shadow) -> Self {
        self.style_mut().shadows.push(s);
        self
    }
    fn opacity(mut self, o: f32) -> Self {
        self.style_mut().opacity = Some(o);
        self
    }
    fn cursor(mut self, c: CursorIcon) -> Self {
        self.style_mut().cursor = Some(c);
        self
    }

    fn text_color(mut self, c: Color) -> Self {
        self.style_mut().text.color = Some(c);
        self
    }
    fn font_size(mut self, v: f32) -> Self {
        self.style_mut().text.size = Some(v);
        self
    }
    /// 400 is regular, 700 bold.
    fn font_weight(mut self, w: u16) -> Self {
        self.style_mut().text.weight = Some(w);
        self
    }
    fn line_height(mut self, v: f32) -> Self {
        self.style_mut().text.line_height = Some(v);
        self
    }
    /// Text inside stays on one line.
    fn nowrap(mut self) -> Self {
        self.style_mut().text.nowrap = Some(true);
        self
    }

    /// Applies `f` when `cond` holds.
    fn when(self, cond: bool, f: impl FnOnce(Self) -> Self) -> Self {
        if cond { f(self) } else { self }
    }
    /// Applies `f` to the value when there is one.
    fn when_some<T>(self, v: Option<T>, f: impl FnOnce(Self, T) -> Self) -> Self {
        match v {
            Some(v) => f(self, v),
            None => self,
        }
    }
}

impl Styled for Style {
    fn style_mut(&mut self) -> &mut Style {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_inherits_and_line_height_follows_the_size() {
        let parent = ResolvedText::default().inherit(&TextStyle {
            size: Some(20.0),
            color: Some(Color::rgb(0xff0000)),
            ..Default::default()
        });
        assert_eq!(parent.line_height(), 28.0);
        let child = parent.inherit(&TextStyle {
            weight: Some(700),
            line_height: Some(22.0),
            ..Default::default()
        });
        assert_eq!(child.color, Color::rgb(0xff0000));
        assert_eq!(
            (child.size, child.weight, child.line_height()),
            (20.0, 700, 22.0)
        );
        let smaller = child.inherit(&TextStyle {
            size: Some(10.0),
            ..Default::default()
        });
        assert_eq!(smaller.line_height(), 14.0);
    }

    #[test]
    fn hover_changes_only_paint() {
        let mut base = Style::default().pad(8.0).background(Color::rgb(1));
        let hover = Style::default().background(Color::rgb(2)).pad(16.0);
        base.refine_paint(&hover);
        assert_eq!(base.background, Some(Color::rgb(2)));
        assert_eq!(base.padding, Edges::all(8.0));
    }
}

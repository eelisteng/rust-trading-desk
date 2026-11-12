//! Custom egui painters: candlestick + volume + moving averages, and an
//! equity curve for the backtest tab. No chart crate needed.

use egui::{pos2, vec2, Align2, Color32, FontId, Pos2, Rect, Sense, Stroke, Ui};
use tdesk_core::{Candle, MarketRules, Money, Price};

use crate::theme::Palette;

fn moving_average(view: &[Candle], period: usize, i: usize) -> Option<f64> {
    if i + 1 < period || period == 0 {
        return None;
    }
    let sum: i64 = view[i + 1 - period..=i].iter().map(|c| c.close).sum();
    Some(sum as f64 / period as f64)
}

pub fn candle_chart(
    ui: &mut Ui,
    candles: &[Candle],
    rules: &dyn MarketRules,
    palette: &Palette,
    visible_bars: usize,
    show_ma: bool,
    symbol: &str,
) {
    let size = vec2(ui.available_width(), ui.available_height());
    let (response, painter) = ui.allocate_painter(size, Sense::hover());
    let rect = response.rect;

    painter.rect_filled(rect, 4.0, palette.panel);
    let n = visible_bars.min(candles.len());
    if n == 0 {
        painter.text(
            rect.center(),
            Align2::CENTER_CENTER,
            "暂无行情数据",
            FontId::proportional(14.0),
            palette.muted,
        );
        return;
    }
    let view = &candles[candles.len() - n..];
    let prev_close = if candles.len() > n {
        candles[candles.len() - n - 1].close
    } else {
        view[0].open
    };
    let last = view.last().map(|c| c.close).unwrap_or(prev_close);
    let change = last - prev_close;
    let change_pct = if prev_close == 0 {
        0.0
    } else {
        change as f64 / prev_close as f64 * 100.0
    };

    let mut low = Price::MAX;
    let mut high = Price::MIN;
    let mut max_volume = 1_i64;
    for c in view {
        low = low.min(c.low);
        high = high.max(c.high);
        max_volume = max_volume.max(c.volume);
    }
    let padding = ((high - low) as f64 * 0.08).round().max(1.0) as Price;
    low = (low - padding).max(1);
    high += padding;
    let span = (high - low).max(1) as f64;

    let left = rect.left() + 10.0;
    let right = rect.right() - 74.0;
    let plot_w = (right - left).max(20.0);
    let top = rect.top() + 30.0;
    let bottom = rect.bottom() - 20.0;
    let price_h = ((bottom - top) * 0.72).max(30.0);
    let volume_top = top + price_h + 16.0;
    let volume_bottom = (bottom - 12.0).max(volume_top + 10.0);
    let step = plot_w / n as f32;
    let body_w = (step * 0.62).clamp(1.0, 16.0);
    let y = |p: Price| -> f32 { top + ((high - p) as f64 / span) as f32 * price_h };
    let x = |i: usize| -> f32 { left + (i as f32 + 0.5) * step };

    // Header
    let header = format!(
        "{symbol}    {}    {:+.2} ({:+.2}%)    {} 根K线",
        rules.format_price(last),
        rules.price_to_f64(change),
        change_pct,
        n
    );
    painter.text(
        pos2(left, rect.top() + 8.0),
        Align2::LEFT_TOP,
        header,
        FontId::proportional(13.0),
        palette.change_color(change as f64),
    );
    if show_ma {
        painter.text(
            pos2(right - 10.0, rect.top() + 9.0),
            Align2::RIGHT_TOP,
            "MA5",
            FontId::proportional(10.0),
            palette.ma_fast,
        );
        painter.text(
            pos2(right - 46.0, rect.top() + 9.0),
            Align2::RIGHT_TOP,
            "MA20",
            FontId::proportional(10.0),
            palette.ma_slow,
        );
    }

    // Horizontal grid + right price axis
    for k in 0..=4 {
        let p = low + ((high - low) * k / 4);
        let yy = y(p);
        painter.line_segment(
            [pos2(left, yy), pos2(right, yy)],
            Stroke::new(1.0_f32, palette.grid),
        );
        painter.text(
            pos2(right + 6.0, yy),
            Align2::LEFT_CENTER,
            rules.format_price(p),
            FontId::monospace(10.0),
            palette.muted,
        );
    }

    // Candles + volume
    for (i, c) in view.iter().enumerate() {
        let reference = if i > 0 { view[i - 1].close } else { prev_close };
        let color = palette.change_color((c.close - reference) as f64);
        let xx = x(i);
        painter.line_segment(
            [pos2(xx, y(c.high)), pos2(xx, y(c.low))],
            Stroke::new(1.0_f32, color),
        );
        let yo = y(c.open);
        let yc = y(c.close);
        let body = Rect::from_min_max(
            pos2(xx - body_w / 2.0, yo.min(yc)),
            pos2(xx + body_w / 2.0, yo.max(yc).max(yo.min(yc) + 1.0)),
        );
        painter.rect_filled(body, 0.0, color);
        let vh = (c.volume as f64 / max_volume as f64) as f32
            * (volume_bottom - volume_top);
        painter.rect_filled(
            Rect::from_min_max(
                pos2(xx - body_w / 2.0, volume_bottom - vh.max(1.0)),
                pos2(xx + body_w / 2.0, volume_bottom),
            ),
            0.0,
            Color32::from_rgba_premultiplied(color.r(), color.g(), color.b(), 150),
        );
    }
    painter.line_segment(
        [pos2(left, volume_bottom), pos2(right, volume_bottom)],
        Stroke::new(1.0_f32, palette.grid),
    );
    painter.text(
        pos2(left, volume_top - 12.0),
        Align2::LEFT_TOP,
        "成交量",
        FontId::proportional(10.0),
        palette.muted,
    );

    // Moving averages
    if show_ma {
        for (period, color) in [(5_usize, palette.ma_fast), (20_usize, palette.ma_slow)] {
            let mut previous: Option<Pos2> = None;
            for i in 0..n {
                if let Some(ma) = moving_average(view, period, i) {
                    let point = pos2(x(i), y(ma.round() as Price));
                    if let Some(prev) = previous {
                        painter.line_segment([prev, point], Stroke::new(1.4_f32, color));
                    }
                    previous = Some(point);
                }
            }
        }
    }

    // Latest price dashed line
    let ly = y(last);
    let mut cursor = left;
    while cursor < right {
        painter.line_segment(
            [pos2(cursor, ly), pos2((cursor + 6.0).min(right), ly)],
            Stroke::new(1.0_f32, palette.accent),
        );
        cursor += 10.0;
    }
    painter.rect_filled(
        Rect::from_min_max(pos2(right + 2.0, ly - 9.0), pos2(rect.right() - 2.0, ly + 9.0)),
        2.0,
        palette.accent,
    );
    painter.text(
        pos2(right + 5.0, ly),
        Align2::LEFT_CENTER,
        rules.format_price(last),
        FontId::monospace(10.0),
        Color32::WHITE,
    );

    // Crosshair + OHLC tooltip
    if let Some(hover) = response.hover_pos() {
        if hover.x >= left && hover.x <= right && hover.y >= top && hover.y <= volume_bottom {
            let idx = (((hover.x - left) / step) as usize).min(n - 1);
            let c = view[idx];
            painter.line_segment(
                [pos2(x(idx), top), pos2(x(idx), volume_bottom)],
                Stroke::new(1.0_f32, palette.muted),
            );
            painter.line_segment(
                [pos2(left, hover.y), pos2(right, hover.y)],
                Stroke::new(1.0_f32, palette.muted),
            );
            let tip = pos2(
                (hover.x + 14.0).min(rect.right() - 178.0),
                (hover.y + 14.0).min(rect.bottom() - 86.0),
            );
            painter.rect_filled(
                Rect::from_min_size(tip, vec2(172.0, 82.0)),
                4.0,
                palette.histogram,
            );
            let lines = format!(
                "开 {}\n高 {}\n低 {}\n收 {}\n量 {}",
                rules.format_price(c.open),
                rules.format_price(c.high),
                rules.format_price(c.low),
                rules.format_price(c.close),
                c.volume
            );
            painter.text(
                pos2(tip.x + 8.0, tip.y + 6.0),
                Align2::LEFT_TOP,
                lines,
                FontId::monospace(10.0),
                palette.text,
            );
        }
    }
}

pub fn equity_chart(
    ui: &mut Ui,
    curve: &[Money],
    rules: &dyn MarketRules,
    palette: &Palette,
) {
    let size = vec2(ui.available_width(), ui.available_height());
    let (response, painter) = ui.allocate_painter(size, Sense::hover());
    let rect = response.rect;
    painter.rect_filled(rect, 4.0, palette.panel);
    if curve.len() < 2 {
        painter.text(
            rect.center(),
            Align2::CENTER_CENTER,
            "点击「运行回测」生成权益曲线",
            FontId::proportional(13.0),
            palette.muted,
        );
        return;
    }
    let min = curve.iter().copied().min().unwrap_or(0);
    let max = curve.iter().copied().max().unwrap_or(1);
    let pad = ((max - min) as f64 * 0.1).round().max(100.0) as Money;
    let lo = min - pad;
    let hi = max + pad;
    let left = rect.left() + 12.0;
    let right = rect.right() - 88.0;
    let top = rect.top() + 20.0;
    let bottom = rect.bottom() - 16.0;
    let y = |v: Money| -> f32 { bottom - (v - lo) as f32 / (hi - lo).max(1) as f32 * (bottom - top) };
    let x = |i: usize| -> f32 { left + i as f32 / (curve.len() - 1) as f32 * (right - left) };

    for k in 0..=4 {
        let v = lo + (hi - lo) * k / 4;
        let yy = y(v);
        painter.line_segment(
            [pos2(left, yy), pos2(right, yy)],
            Stroke::new(1.0_f32, palette.grid),
        );
        painter.text(
            pos2(right + 6.0, yy),
            Align2::LEFT_CENTER,
            rules.format_money(v),
            FontId::monospace(10.0),
            palette.muted,
        );
    }
    let base = y(curve[0]);
    for i in 1..curve.len() {
        let color = if curve[i] >= curve[i - 1] {
            palette.up
        } else {
            palette.down
        };
        painter.line_segment(
            [pos2(x(i - 1), y(curve[i - 1])), pos2(x(i), y(curve[i]))],
            Stroke::new(1.6_f32, color),
        );
        painter.line_segment(
            [pos2(x(i), base), pos2(x(i), y(curve[i]))],
            Stroke::new(0.5_f32, Color32::from_rgba_premultiplied(color.r(), color.g(), color.b(), 45)),
        );
    }
    painter.text(
        pos2(left, rect.top() + 4.0),
        Align2::LEFT_TOP,
        format!("权益曲线  {} 个交易日", curve.len()),
        FontId::proportional(11.0),
        palette.text,
    );
}

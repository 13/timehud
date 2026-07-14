"""
widgets.py – Small custom widgets and styling helpers for the overlay.
"""

from PyQt6.QtCore import Qt, QVariantAnimation
from PyQt6.QtGui import QColor, QFont, QPainter
from PyQt6.QtWidgets import QWidget


def rgba(hex_color: str, alpha: float) -> str:
    """'#RRGGBB' + 0-1 alpha → Qt stylesheet rgba() string."""
    h = hex_color.lstrip("#")
    r, g, b = (int(h[i:i + 2], 16) for i in (0, 2, 4))
    return f"rgba({r},{g},{b},{int(alpha * 255)})"


def tabular(font: QFont) -> QFont:
    """Enable tabular (fixed-width) digits where supported (Qt >= 6.7)."""
    try:
        font.setFeature(QFont.Tag("tnum"), 1)
    except (AttributeError, TypeError):
        pass  # older Qt: mono fonts are tabular anyway
    return font


class ProgressBar(QWidget):
    """Thin rounded bar showing remaining fraction of a countdown/phase.

    Successive tick fractions are interpolated over ~one tick period so the
    fill moves continuously instead of stepping at 10 fps.
    """

    def __init__(self, parent=None) -> None:
        super().__init__(parent)
        self._fraction = -1.0          # latest target from the engine
        self._display_fraction = -1.0  # what paintEvent draws (animated)
        self._color = QColor("#FFFFFF")
        self._anim: QVariantAnimation | None = None
        self.setFixedHeight(3)
        self.hide()

    def set_state(self, fraction: float, color: str) -> None:
        qc = QColor(color)
        # Visibility before the dedup return: update_ui may have hidden the
        # bar while show_timer was off, with fraction/color unchanged.
        self.setVisible(fraction >= 0.0)
        if fraction == self._fraction and qc == self._color:
            return
        previous = self._display_fraction
        self._fraction = fraction
        self._color = qc
        if self._anim is not None:
            self._anim.stop()
            self._anim.deleteLater()
            self._anim = None
        # Snap on hide/reset/large jumps; glide between adjacent ticks
        if fraction < 0 or previous < 0 or abs(fraction - previous) > 0.2:
            self._display_fraction = fraction
            self.update()
            return
        anim = QVariantAnimation(self)
        anim.setDuration(120)
        anim.setStartValue(previous)
        anim.setEndValue(fraction)
        anim.valueChanged.connect(self._on_anim_step)
        anim.start()
        self._anim = anim

    def _on_anim_step(self, value) -> None:
        self._display_fraction = float(value)
        self.update()

    def paintEvent(self, _event) -> None:  # noqa: N802
        if self._display_fraction < 0:
            return
        p = QPainter(self)
        p.setRenderHint(QPainter.RenderHint.Antialiasing)
        p.setPen(Qt.PenStyle.NoPen)
        p.setBrush(QColor(255, 255, 255, 30))
        p.drawRoundedRect(self.rect(), 1.5, 1.5)
        w = int(self.width() * min(1.0, self._display_fraction))
        if w > 0:
            p.setBrush(self._color)
            p.drawRoundedRect(0, 0, w, self.height(), 1.5, 1.5)

# Stroke packs

The strokes Arin writes scripts other than its hand's in. Each pack was converted by
`packaging/strokes/build.py` from an open source that recorded how each character is written,
and each carries a notice inside it saying where it came from, under what licence, and how and
when it was changed. They are data shipped beside Arin, not part of its code, and each keeps
its own licence.

| Pack | Source | Licence |
|---|---|---|
| `greek-cyrillic.strk`, `maths.strk` | KiCad's newstroke font, by Vladimir Uryvaev | CC0 |
| `japanese.strk` | KanjiVG, copyright Ulrich Apel, with newstroke punctuation | CC BY-SA 3.0, which this adaptation is distributed under |
| `chinese-simplified.strk` | Make Me a Hanzi, derived from Arphic PL KaitiM GB and UKai, with newstroke punctuation | Arphic Public License, in `ARPHICPL.TXT` |
| `chinese-traditional.strk` | AnimCJK, derived from Arphic fonts, with newstroke punctuation | Arphic Public License, in `ARPHICPL.TXT` |

The conversion is in `packaging/strokes/build.py`, which is where the modifications the Arphic
licence asks to be freely available are. Run `just handwriting-data` to rebuild every pack
from its pinned source.

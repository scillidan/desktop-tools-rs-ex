# /// script
# requires-python = ">=3.8"
# dependencies = ["rapidocr-onnxruntime"]
# ///
# Persistent RapidOCR worker. Provenance: keytools-rs apps/screen-ocr/lib/rapidocr_cli.py.
# Protocol: request `<lang>\t<image.png>\n`; response BEGIN, word rows, END.
# Word row: `<l>,<t>,<r>,<b>\t<line-index>\t<text>\n`.
import sys


def clean(text):
    return str(text).replace("\t", " ").replace("\r", " ").replace("\n", " ")


def emit_word(text, points, line_index):
    text = clean(text).strip()
    if not text or not points:
        return
    if (
        points
        and isinstance(points[0], (list, tuple))
        and points[0]
        and isinstance(points[0][0], (list, tuple))
    ):
        points = [point for box in points for point in box]
    xs = [float(p[0]) for p in points]
    ys = [float(p[1]) for p in points]
    box = "%d,%d,%d,%d" % (
        round(min(xs)),
        round(min(ys)),
        round(max(xs)),
        round(max(ys)),
    )
    print("%s\t%d\t%s" % (box, line_index, text), flush=True)


def cjk(char):
    n = ord(char)
    return (
        0x3400 <= n <= 0x9FFF
        or 0xF900 <= n <= 0xFAFF
        or 0x3040 <= n <= 0x30FF
        or 0xAC00 <= n <= 0xD7AF
    )


def emit_chars(chars, boxes, line_index):
    """Group native character boxes into clickable words without guessing widths."""
    chars_out = []
    boxes_out = []

    def flush():
        if chars_out:
            emit_word("".join(chars_out), boxes_out, line_index)
            chars_out.clear()
            boxes_out.clear()

    for char, box in zip(chars, boxes):
        if char.isspace():
            flush()
        elif cjk(char):
            flush()
            emit_word(char, box, line_index)
        else:
            chars_out.append(char)
            boxes_out.append(box)
    flush()


def emit_result(result):
    print("BEGIN", flush=True)
    for line_index, row in enumerate(result or []):
        if len(row) < 2:
            continue
        # Current ONNX models: [line box, text, score, char boxes, chars, scores].
        if len(row) > 4 and isinstance(row[3], list) and isinstance(row[4], list):
            emit_chars(row[4], row[3], line_index)
            continue
        # Other RapidOCR versions: row[3] is [(word, confidence, box), ...].
        words = row[3] if len(row) > 3 else None
        if (
            words
            and isinstance(words[0], (list, tuple))
            and len(words[0]) >= 3
            and isinstance(words[0][0], str)
        ):
            for item in words:
                emit_word(item[0], item[2], line_index)
            continue
        # No character/word boxes: retain the recognized line as one word.
        emit_word(row[1], row[0], line_index)
    print("END", flush=True)


def main():
    if len(sys.argv) != 2 or sys.argv[1] != "--worker":
        raise SystemExit("usage: rapidocr_worker.py --worker")
    sys.stdin.reconfigure(encoding="utf-8")
    sys.stdout.reconfigure(encoding="utf-8")
    from rapidocr_onnxruntime import RapidOCR

    engines = {}
    for raw in sys.stdin:
        request = raw.rstrip("\r\n")
        if not request:
            continue
        try:
            lang, image_path = request.split("\t", 1)
            engine = engines.get(lang)
            if engine is None:
                engine = RapidOCR(lang=lang, use_angle_cls=False)
                engines[lang] = engine
            result, _ = engine(image_path, return_word_box=True)
            emit_result(result)
        except Exception as exc:
            message = clean(exc)
            print("ERROR\t" + message, flush=True)


if __name__ == "__main__":
    main()

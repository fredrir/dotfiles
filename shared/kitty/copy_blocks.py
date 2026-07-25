import os
import sys
from typing import Any

sys.dont_write_bytecode = True
sys.path.insert(
    0,
    os.environ.get("XDG_CONFIG_HOME", os.path.expanduser("~/.config")),
)

from kitty.clipboard import set_clipboard_string
from kittens.tui.handler import result_handler
from terminal_blocks import select_blocks


def main(args: list[str]) -> dict[str, object]:
    requested = args[1] if len(args) > 1 else None
    selection = select_blocks(sys.stdin.read(), requested)
    return {
        "text": selection.text,
        "count": selection.count,
        "clear_input": selection.clear_input,
    }


@result_handler(type_of_input="screen-history", no_ui=True)
def handle_result(
    args: list[str],
    data: dict[str, object],
    target_window_id: int,
    boss: Any,
) -> None:
    text = data.get("text")
    if not isinstance(text, str) or not text:
        return
    set_clipboard_string(text)
    boss.handle_clipboard_loss("clipboard")
    if data.get("clear_input"):
        window = boss.window_id_map.get(target_window_id)
        if window is not None:
            window.write_to_child(b"\x15")

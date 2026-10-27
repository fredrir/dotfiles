import { commands, Position, Selection, window } from "vscode";

export async function rename(
  uri: string,
  position: { line: number; character: number },
): Promise<void> {
  const editor = window.activeTextEditor;
  if (editor?.document.uri.toString() !== uri) {
    return;
  }
  const target = new Position(position.line, position.character);
  editor.selection = new Selection(target, target);
  await commands.executeCommand("editor.action.rename");
}

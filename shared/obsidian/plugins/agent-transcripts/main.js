const { FuzzySuggestModal, Menu, Notice, Plugin, TFile, setIcon } = require("obsidian");
const {
  formatLocalDate,
  markTranscriptTurn,
  swapProviderText,
  wrapAgentTranscript,
} = require("./format");

const COPY_KINDS = new Set(["me", "turn", "tool", "claude", "codex", "agent", "chatgpt"]);
const PROVIDERS = ["claude", "codex", "chatgpt", "agent"];
const PROVIDER_KINDS = new Set(PROVIDERS);

const PROVIDER_CHOICES = [
  { label: "Claude", value: "claude" },
  { label: "Codex", value: "codex" },
  { label: "ChatGPT", value: "chatgpt" },
  { label: "Agent", value: "agent" },
];

const TURN_CHOICES = [
  { label: "Me — user message", value: "me" },
  { label: "Turn — assistant reply", value: "turn" },
  { label: "Tool — command / output", value: "tool" },
];

class ChoiceModal extends FuzzySuggestModal {
  constructor(app, items, placeholder, onChoose) {
    super(app);
    this.items = items;
    this.onChoose = onChoose;
    this.setPlaceholder(placeholder);
  }

  getItems() {
    return this.items;
  }

  getItemText(item) {
    return item.label;
  }

  onChooseItem(item) {
    void this.onChoose(item.value);
  }
}

function textOf(content) {
  const clone = content.cloneNode(true);
  clone.style.position = "fixed";
  clone.style.left = "-99999px";
  clone.style.top = "0";
  clone.style.display = "block";
  document.body.appendChild(clone);
  const text = clone.innerText;
  clone.remove();
  return text.trim();
}

module.exports = class AgentTranscripts extends Plugin {
  onload() {
    this.addCommand({
      id: "wrap-agent-transcript",
      name: "Wrap selection as agent transcript",
      editorCallback: (editor) => this.openWrapPicker(editor),
    });
    this.addCommand({
      id: "mark-transcript-turn",
      name: "Mark selection as transcript turn",
      editorCallback: (editor) => this.openTurnPicker(editor),
    });

    this.registerMarkdownPostProcessor((element, context) => {
      for (const callout of element.querySelectorAll(".callout")) {
        const kind = callout.getAttribute("data-callout");
        if (!COPY_KINDS.has(kind)) continue;
        const title = callout.querySelector(".callout-title");
        const content = callout.querySelector(".callout-content");
        if (!title || title.querySelector(".agent-copy-button")) continue;
        if (content) title.appendChild(this.copyButton(content));
        if (PROVIDER_KINDS.has(kind)) {
          title.appendChild(this.providerButton(kind, context.sourcePath));
        }
      }
    });
  }

  openWrapPicker(editor) {
    const from = editor.getCursor("from");
    const to = editor.getCursor("to");
    const selection = editor.getRange(from, to);
    new ChoiceModal(this.app, PROVIDER_CHOICES, "Wrap paste as…", async (provider) => {
      let text = selection;
      if (!text.trim()) {
        try {
          text = await navigator.clipboard.readText();
        } catch (error) {
          console.error("Agent Transcripts: could not read clipboard", error);
          new Notice("Could not read the clipboard");
          return;
        }
      }
      if (!text.trim()) {
        new Notice("The selection and clipboard are empty");
        return;
      }
      editor.replaceRange(wrapAgentTranscript(text, provider, formatLocalDate(new Date())), from, to);
    }).open();
  }

  openTurnPicker(editor) {
    const from = editor.getCursor("from");
    const to = editor.getCursor("to");
    const selection = editor.getRange(from, to);
    new ChoiceModal(this.app, TURN_CHOICES, "Mark selection as…", (kind) => {
      if (!selection) return;
      editor.replaceRange(markTranscriptTurn(selection, kind), from, to);
    }).open();
  }

  copyButton(content) {
    const button = document.createElement("button");
    button.className = "agent-copy-button";
    button.setAttribute("aria-label", "Copy contents");
    setIcon(button, "copy");
    button.addEventListener("click", (event) => {
      event.preventDefault();
      event.stopPropagation();
      navigator.clipboard.writeText(textOf(content)).then(() => {
        setIcon(button, "check");
        window.setTimeout(() => setIcon(button, "copy"), 1200);
      }).catch((error) => {
        console.error("Agent Transcripts: could not write clipboard", error);
        new Notice("Could not copy transcript contents");
      });
    });
    return button;
  }

  providerButton(current, sourcePath) {
    const button = document.createElement("button");
    button.className = "agent-provider-button";
    button.setAttribute("aria-label", "Set provider");
    setIcon(button, "replace");
    button.addEventListener("click", (event) => {
      event.preventDefault();
      event.stopPropagation();
      const menu = new Menu();
      for (const provider of PROVIDERS) {
        menu.addItem((item) =>
          item
            .setTitle(provider)
            .setChecked(provider === current)
            .setDisabled(provider === current)
            .onClick(() => this.reassign(sourcePath, current, provider)),
        );
      }
      menu.showAtMouseEvent(event);
    });
    return button;
  }

  async reassign(sourcePath, from, to) {
    const file = this.app.vault.getAbstractFileByPath(sourcePath);
    if (!(file instanceof TFile)) return;
    await this.app.vault.process(file, (text) => swapProviderText(text, from, to));
    const parts = file.path.split("/");
    const index = parts.length - 2;
    if (index >= 0 && PROVIDER_KINDS.has(parts[index]) && parts[index] !== to) {
      parts[index] = to;
      let target = parts.join("/");
      const folder = parts.slice(0, -1).join("/");
      if (!this.app.vault.getAbstractFileByPath(folder)) {
        await this.app.vault.createFolder(folder).catch(() => {});
      }
      if (this.app.vault.getAbstractFileByPath(target)) {
        target = target.replace(/\.md$/, " (1).md");
      }
      await this.app.fileManager.renameFile(file, target);
    }
    new Notice(`Provider set to ${to}`);
  }
};

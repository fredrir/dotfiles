const { Menu, Notice, Plugin, TFile, setIcon } = require("obsidian");

const COPY_KINDS = new Set(["me", "turn", "tool", "claude", "codex", "agent", "chatgpt"]);
const PROVIDERS = ["claude", "codex", "chatgpt", "agent"];
const PROVIDER_KINDS = new Set(PROVIDERS);

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

function swapProviderText(text, from, to) {
  return text
    .replace(new RegExp("(^[>\\s]*\\[!)" + from + "(\\]|\\|)", "gm"), "$1" + to + "$2")
    .replace(new RegExp("(\\[!turn\\|)" + from + "(\\])", "g"), "$1" + to + "$2")
    .replace(/^provider:.*$/m, "provider: " + to);
}

module.exports = class AgentCards extends Plugin {
  onload() {
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
            .onClick(() => this.reassign(sourcePath, current, provider))
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
    new Notice("Provider set to " + to);
  }
};

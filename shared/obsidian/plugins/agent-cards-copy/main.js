const { Plugin, setIcon } = require("obsidian");

const KINDS = new Set(["me", "turn", "tool", "claude", "codex", "agent", "chatgpt"]);

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

module.exports = class AgentCardsCopy extends Plugin {
  onload() {
    this.registerMarkdownPostProcessor((element) => {
      for (const callout of element.querySelectorAll(".callout")) {
        if (!KINDS.has(callout.getAttribute("data-callout"))) continue;
        const title = callout.querySelector(".callout-title");
        const content = callout.querySelector(".callout-content");
        if (!title || !content || title.querySelector(".agent-copy-button")) continue;
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
        title.appendChild(button);
      }
    });
  }
};

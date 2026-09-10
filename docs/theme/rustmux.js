(function () {
  function enhanceBrand() {
    const title = document.querySelector(".menu-title");
    if (!title) return;
    title.innerHTML = `
      <span class="rustmux-brand">
        <span class="rustmux-mark" aria-hidden="true">R</span>
        <span>Rustmux</span>
        <span class="rustmux-docs-label">Docs</span>
        <span class="rustmux-version">v0.1</span>
      </span>`;
  }

  function addCopyButtons() {
    for (const pre of document.querySelectorAll("main pre")) {
      const code = pre.querySelector("code");
      if (!code || pre.querySelector(".copy-button")) continue;
      const button = document.createElement("button");
      button.className = "copy-button";
      button.type = "button";
      button.textContent = "COPY";
      button.setAttribute("aria-label", "复制代码");
      button.addEventListener("click", async () => {
        await navigator.clipboard.writeText(code.textContent || "");
        button.textContent = "COPIED";
        window.setTimeout(() => (button.textContent = "COPY"), 1200);
      });
      pre.append(button);
    }
  }

  function addTableOfContents() {
    const headings = [...document.querySelectorAll("main h2, main h3")];
    if (headings.length < 2) return;
    const aside = document.createElement("aside");
    aside.className = "right-side-toc";
    aside.setAttribute("aria-label", "本页内容");
    aside.innerHTML = "<strong>本页内容</strong>";

    const links = headings.map((heading) => {
      const link = document.createElement("a");
      link.href = `#${heading.id}`;
      link.textContent = heading.textContent || "";
      link.dataset.level = heading.tagName.slice(1);
      aside.append(link);
      return link;
    });
    document.body.append(aside);

    const observer = new IntersectionObserver(
      (entries) => {
        const visible = entries.find((entry) => entry.isIntersecting);
        if (!visible) return;
        for (const link of links) link.classList.toggle("active", link.hash === `#${visible.target.id}`);
      },
      { rootMargin: "-20% 0px -70% 0px" },
    );
    headings.forEach((heading) => observer.observe(heading));
  }

  function bindSearchShortcut() {
    document.addEventListener("keydown", (event) => {
      if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === "k") {
        event.preventDefault();
        document.querySelector("#search-toggle")?.click();
      }
    });
  }

  function markExternalLinks() {
    for (const link of document.querySelectorAll("main a[href^='http']")) {
      link.target = "_blank";
      link.rel = "noreferrer";
    }
  }

  document.addEventListener("DOMContentLoaded", () => {
    enhanceBrand();
    addCopyButtons();
    addTableOfContents();
    bindSearchShortcut();
    markExternalLinks();
  });
})();

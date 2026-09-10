(function () {
  function isChinesePage() {
    return document.documentElement.lang.toLowerCase().startsWith("zh") ||
      window.location.pathname === "/zh" ||
      window.location.pathname.startsWith("/zh/");
  }

  function markPage() {
    if (document.querySelector("main .landing-hero")) {
      document.body.classList.add("home-page");
    }
  }

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

  function addLanguageSwitch() {
    const actions = document.querySelector(".right-buttons");
    if (!actions) return;
    const path = window.location.pathname;
    const chinese = isChinesePage();
    const targetPath = chinese ? path.replace(/^\/zh(?=\/|$)/, "") || "/" : `/zh${path}`;
    const link = document.createElement("a");
    link.className = "language-switch";
    link.href = `${targetPath}${window.location.search}${window.location.hash}`;
    link.hreflang = chinese ? "en" : "zh-CN";
    link.textContent = chinese ? "EN" : "中文";
    link.setAttribute("aria-label", chinese ? "Read in English" : "阅读中文版");
    actions.prepend(link);
  }

  function addCopyButtons() {
    for (const pre of document.querySelectorAll("main pre")) {
      const code = pre.querySelector("code");
      if (!code || pre.closest(".terminal-window") || pre.querySelector(".copy-button")) continue;
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
    const label = isChinesePage() ? "本页内容" : "On this page";
    aside.setAttribute("aria-label", label);
    const title = document.createElement("strong");
    title.textContent = label;
    aside.append(title);

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
    markPage();
    enhanceBrand();
    addLanguageSwitch();
    addCopyButtons();
    addTableOfContents();
    bindSearchShortcut();
    markExternalLinks();
  });
})();

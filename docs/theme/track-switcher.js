(function () {
  const metadata = document.getElementById("rustmux-tracks");
  if (!metadata) return;
  const config = JSON.parse(metadata.textContent);
  const root = new URL(config.root, location.href);
  const title = document.querySelector(".menu-title");
  if (!title) return;

  const label = document.createElement("label");
  label.className = "track-picker";
  const select = document.createElement("select");
  select.setAttribute("aria-label", "Documentation branch");
  for (const track of ["main", "main-human"]) {
    const option = document.createElement("option");
    option.value = track;
    option.textContent = track;
    option.selected = track === config.track;
    select.append(option);
  }
  label.append(select);
  title.after(label);
  select.addEventListener("change", () => {
    const available = config.pages[select.value].includes(config.page);
    const prefix = select.value === "main" ? "" : "main-human/";
    const target = new URL(prefix + (available ? config.page : "index.html"), root);
    if (available) target.hash = location.hash;
    else target.searchParams.set("missing-page", config.page);
    location.assign(target.href);
  });

  const missing = new URL(location.href).searchParams.get("missing-page");
  if (missing) {
    const notice = document.createElement("p");
    notice.className = "track-notice";
    notice.setAttribute("role", "status");
    notice.textContent = `The page “${missing}” is not available on ${config.track}. Showing this branch’s home page.`;
    document.querySelector("main")?.prepend(notice);
  }
})();

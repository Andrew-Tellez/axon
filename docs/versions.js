// Version selector for the documentation.
//
// The site is rebuilt whole from git's tags on every deploy: the root is `main`
// and each tag lives under its prefix. `versions.json` at the root says which
// ones exist, so there is no external state and no branch to drift.
(async () => {
  const parts = location.pathname.split("/").filter(Boolean);
  const isTag = (s) => /^v\d+\.\d+\.\d+/.test(s);
  const current = [...parts].reverse().find(isTag) ?? "main";
  const root = current === "main"
    ? location.pathname.slice(0, location.pathname.indexOf(parts.at(-1) ?? "") || undefined)
    : location.pathname.slice(0, location.pathname.indexOf(current));

  let data;
  try {
    const r = await fetch(root + "versions.json", { cache: "no-cache" });
    if (!r.ok) throw new Error(r.status);
    data = await r.json();
  } catch {
    // with no versions.json, no made-up list gets shown
    return;
  }

  const bar = document.querySelector(".right-buttons") ?? document.querySelector(".menu-bar");
  if (!bar) return;

  const sel = document.createElement("select");
  sel.className = "axon-versions";
  sel.setAttribute("aria-label", "Documentation version");
  for (const v of data.versions ?? ["main"]) {
    const o = document.createElement("option");
    o.value = v;
    o.textContent = v === "main" ? "main (unreleased)" : v;
    o.selected = v === current;
    sel.append(o);
  }
  sel.onchange = () => {
    location.href = root + (sel.value === "main" ? "" : sel.value + "/");
  };
  bar.append(sel);

  // The first in the list is the newest: if it is not the one being viewed, say so
  const latest = (data.versions ?? []).find((v) => v !== "main");
  if (latest && current !== "main" && current !== latest) {
    const notice = document.createElement("div");
    notice.className = "axon-outdated";
    notice.innerHTML =
      `You are reading the documentation for <strong>${current}</strong>. ` +
      `<a href="${root + latest + "/"}">Go to the latest</a>.`;
    document.querySelector("#content")?.prepend(notice);
  }
})();

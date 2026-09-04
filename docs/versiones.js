// Selector de versiones para la documentacion.
//
// El sitio se reconstruye entero desde los tags de git en cada deploy: la raiz
// es `main` y cada tag queda bajo su prefijo. `versions.json` en la raiz dice
// cuales existen, asi que no hay estado externo ni rama que se desincronice.
(async () => {
  const partes = location.pathname.split("/").filter(Boolean);
  const esTag = (s) => /^v\d+\.\d+\.\d+/.test(s);
  const actual = [...partes].reverse().find(esTag) ?? "main";
  const raiz = actual === "main"
    ? location.pathname.slice(0, location.pathname.indexOf(partes.at(-1) ?? "") || undefined)
    : location.pathname.slice(0, location.pathname.indexOf(actual));

  let datos;
  try {
    const r = await fetch(raiz + "versions.json", { cache: "no-cache" });
    if (!r.ok) throw new Error(r.status);
    datos = await r.json();
  } catch {
    // sin versions.json no se muestra una lista inventada
    return;
  }

  const barra = document.querySelector(".right-buttons") ?? document.querySelector(".menu-bar");
  if (!barra) return;

  const sel = document.createElement("select");
  sel.className = "axon-versiones";
  sel.setAttribute("aria-label", "Version de la documentacion");
  for (const v of datos.versiones ?? ["main"]) {
    const o = document.createElement("option");
    o.value = v;
    o.textContent = v === "main" ? "main (sin liberar)" : v;
    o.selected = v === actual;
    sel.append(o);
  }
  sel.onchange = () => {
    location.href = raiz + (sel.value === "main" ? "" : sel.value + "/");
  };
  barra.append(sel);

  // La primera de la lista es la mas nueva: si no es la que se esta viendo, decirlo
  const ultima = (datos.versiones ?? []).find((v) => v !== "main");
  if (ultima && actual !== "main" && actual !== ultima) {
    const aviso = document.createElement("div");
    aviso.className = "axon-obsoleta";
    aviso.innerHTML =
      `Estás viendo la documentación de <strong>${actual}</strong>. ` +
      `<a href="${raiz + ultima + "/"}">Ir a la última</a>.`;
    document.querySelector("#content")?.prepend(aviso);
  }
})();

//! Colores para la salida. Sin dependencias: son cuatro secuencias ANSI.
//!
//! Azul informa, amarillo advierte, rojo bloquea. La jerarquia importa mas que
//! el color: en una lista de treinta lineas, lo que hay que arreglar tiene que
//! saltar sin leerla entera.
//!
//! Se apagan solos cuando la salida no es una terminal —un pipe, un log de CI,
//! un archivo— porque ahi las secuencias son basura que ensucia el diff. Y
//! respetan `NO_COLOR`, que es la convencion, y `CLICOLOR_FORCE` para el caso
//! contrario.
use std::io::IsTerminal;
use std::sync::OnceLock;

fn activos() -> bool {
    static A: OnceLock<bool> = OnceLock::new();
    *A.get_or_init(|| {
        if std::env::var_os("NO_COLOR").is_some() {
            return false;
        }
        if std::env::var_os("CLICOLOR_FORCE").is_some() {
            return true;
        }
        // stderr y stdout se juzgan juntos: mezclar coloreado y plano en el
        // mismo reporte se lee peor que no colorear nada
        std::io::stdout().is_terminal() && std::io::stderr().is_terminal()
    })
}

fn pintar(codigo: &str, texto: &str) -> String {
    if activos() {
        format!("\x1b[{codigo}m{texto}\x1b[0m")
    } else {
        texto.to_string()
    }
}

/// Rojo: hay que corregirlo antes de seguir.
pub fn rojo(t: &str) -> String {
    pintar("1;31", t)
}
/// Amarillo: conviene mirarlo, no bloquea.
pub fn amarillo(t: &str) -> String {
    pintar("1;33", t)
}
/// Azul: informacion y consejo.
pub fn azul(t: &str) -> String {
    pintar("1;34", t)
}
/// Verde: salio bien.
pub fn verde(t: &str) -> String {
    pintar("1;32", t)
}
/// Gris: contexto secundario, para que no compita con lo importante.
pub fn gris(t: &str) -> String {
    pintar("2", t)
}
/// Negrita sin color, para resaltar un nombre dentro de una frase.
pub fn fuerte(t: &str) -> String {
    pintar("1", t)
}

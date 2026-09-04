//! `pgdog.toml` derivado del manifiesto.
//!
//! Lo que hace verificable a este generador es que pgdog publica el JSON Schema
//! de su configuracion, generado desde sus propios tipos de Rust y comprobado
//! por su CI. Asi que la salida no se compara contra un texto esperado: se
//! valida contra el esquema real del parser que la va a leer.
use crate::manifest::*;

/// Nombre logico de la base para el sharder: el mismo para todos los nodos,
/// porque el cliente se conecta a UNO y pgdog decide el nodo.
fn base(svc: &str) -> String {
    svc.to_string()
}

/// El tipo de la clave de reparto, en el vocabulario de pgdog.
///
/// Su enum admite `bigint`, `uuid`, `vector` y `varchar`: si la columna es de
/// otro tipo, el sharder no sabe hashearla.
fn tipo_clave(t: &Tabla, clave: &str) -> Option<&'static str> {
    let c = t.col(clave)?;
    let ty = c.ty.as_str();
    Some(if ty.starts_with("uuid") {
        "uuid"
    } else if ty.starts_with("bigint") || ty.starts_with("int") || ty.starts_with("smallint") {
        "bigint"
    } else if ty.starts_with("varchar") || ty.starts_with("text") || ty.starts_with("char") {
        "varchar"
    } else {
        return None;
    })
}

pub fn build(ms: &[Manifest]) -> Result<String, String> {
    let activos: Vec<&Manifest> = ms
        .iter()
        .filter(|m| !m.external && m.pooler.activo())
        .collect();
    if activos.is_empty() {
        return Err("ningun servicio declara `[pooler] engine`".into());
    }
    let esquemas = schemas(ms);
    let mut o = vec![
        "# generado por axon — no editar.".to_string(),
        "#   axon pooler manifests/ > pgdog.toml".to_string(),
        "#".to_string(),
        "# Los nodos y sus datos de conexion salen de variables de entorno: un".to_string(),
        "# archivo de configuracion generado no es lugar para una contrasena.".to_string(),
        String::new(),
    ];

    for m in activos {
        let pl = &m.pooler;
        let svc = &m.service;
        o.push(format!("# ---------- {svc} ----------"));
        o.push("[general]".into());
        o.push(format!("pooler_mode = \"{}\"", pl.mode));
        if let Some(n) = pl.pool_size {
            o.push(format!("default_pool_size = {n}"));
        }
        // Rechazar antes que devolver un resultado incompleto: sin JOIN entre
        // nodos ni unicidad global, una consulta que cruza se responde mal.
        o.push(format!(
            "# rechaza la consulta que cruza nodos en vez de ejecutarla: sin JOIN entre\n\
             # nodos, lo que el sharder no sabe resolver devolveria un resultado parcial\ncross_shard_disabled = {}",
            pl.cross_shard_disabled
        ));
        if pl.shards > 1 {
            // El parser tiene que estar SIEMPRE encendido: en `auto` no se
            // activa con un solo nodo primario, y ahi es justo donde una GUC de
            // sesion se cuela sin ser interceptada.
            o.push(
                "# `on` y no `auto`: en `auto` el parser no se activa con un solo nodo\n\
                 # primario, que es exactamente el caso donde una GUC de sesion se cuela\nquery_parser = \"on\""
                    .into(),
            );
            o.push(format!(
                "# el reparto se declara en el manifiesto; aca solo se refleja\n# shards = {}",
                pl.shards
            ));
        }
        o.push(String::new());

        // Un bloque `databases` por nodo. Los datos de conexion por variable.
        for shard in 0..pl.shards.max(1) {
            o.push("[[databases]]".into());
            o.push(format!("name = \"{}\"", base(svc)));
            o.push(format!("host = \"${{AXON_DB_HOST_{}}}\"", shard));
            o.push(format!("port = ${{AXON_DB_PORT_{shard}}}"));
            o.push(format!("database_name = \"{svc}\""));
            o.push("role = \"primary\"".into());
            if pl.shards > 1 {
                o.push(format!("shard = {shard}"));
            }
            if let Some(n) = pl.pool_size {
                o.push(format!("pool_size = {n}"));
            }
            o.push(String::new());
        }
        // y las replicas de lectura, si se declararon
        for r in 0..m.infra.read_replicas.unwrap_or(0) {
            o.push("[[databases]]".into());
            o.push(format!("name = \"{}\"", base(svc)));
            o.push(format!("host = \"${{AXON_DB_RO_HOST_{r}}}\""));
            o.push(format!("port = ${{AXON_DB_RO_PORT_{r}}}"));
            o.push(format!("database_name = \"{svc}\""));
            o.push("role = \"replica\"".into());
            o.push(String::new());
        }

        if pl.shards > 1 {
            let clave = m
                .infra
                .shard_key
                .as_ref()
                .ok_or_else(|| format!("{svc}: `shards > 1` sin `shard_key`"))?;
            let tablas = esquemas.get(svc).ok_or_else(|| {
                format!("{svc}: sin migraciones, no se puede declarar el reparto")
            })?;
            let mut declaradas = 0;
            for (t, tb) in tablas {
                if ["outbox", "inbox_seen"].contains(&t.as_str()) || !tb.tiene(clave) {
                    continue;
                }
                let tipo = tipo_clave(tb, clave).ok_or_else(|| {
                    format!(
                        "{svc}.{t}.{clave}: tipo `{}` que el sharder no sabe hashear; admite \
                         uuid, bigint y varchar",
                        tb.col(clave).map(|c| c.ty.as_str()).unwrap_or("?")
                    )
                })?;
                o.push("[[sharded_tables]]".into());
                o.push(format!("database = \"{}\"", base(svc)));
                o.push(format!("name = \"{t}\""));
                o.push(format!("column = \"{clave}\""));
                o.push(format!("data_type = \"{tipo}\""));
                // el mismo hash que `PARTITION BY HASH` de Postgres, para que
                // el reparto coincida si algun dia se mueve dentro del motor
                o.push("hasher = \"postgres\"".into());
                o.push(String::new());
                declaradas += 1;
            }
            if declaradas == 0 {
                return Err(format!(
                    "{svc}: ninguna tabla lleva `{clave}`, asi que no hay nada que repartir"
                ));
            }
        }
    }
    o.push(String::new());
    Ok(o.join("\n"))
}

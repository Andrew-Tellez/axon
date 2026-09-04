//! `pgdog.toml` derivado del manifiesto.
//!
//! Lo que hace verificable a este generador es que pgdog publica el JSON Schema
//! de su configuracion, generado desde sus propios tipos de Rust y comprobado
//! por su CI. Asi que la salida no se compara contra un texto esperado: se
//! valida contra el esquema real del parser que la va a leer.
//!
//! Un archivo POR SERVICIO, no uno para todos. `[general]`, `[multi_tenant]` y
//! `[admin]` son tablas unicas: emitirlas dos veces no es una configuracion
//! discutible, es un TOML que no parsea. Y coincide con base-por-servicio: si
//! dos servicios comparten un pooler, comparten su cola de conexiones.
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

/// El servicio cuyo pooler se va a emitir.
///
/// Si hay uno solo, no hay nada que elegir. Si hay varios, elegir por el
/// usuario seria elegir mal en silencio: cada uno tiene su archivo.
fn elegir<'a>(ms: &'a [Manifest], solo: Option<&str>) -> Result<&'a Manifest, String> {
    let activos: Vec<&Manifest> = ms
        .iter()
        .filter(|m| !m.external && m.pooler.activo())
        .collect();
    match (activos.len(), solo) {
        (0, _) => Err("ningun servicio declara `[pooler] engine`".into()),
        (_, Some(s)) => activos
            .into_iter()
            .find(|m| m.service == s)
            .ok_or_else(|| format!("`{s}` no declara `[pooler] engine`")),
        (1, None) => Ok(activos[0]),
        (_, None) => Err(format!(
            "{} servicios declaran un pooler ({}), y cada uno lleva su propio \
             pgdog.toml: `[general]` es una tabla unica. Eleg con `--service`.",
            activos.len(),
            activos
                .iter()
                .map(|m| m.service.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

/// El host de un nodo: el contenedor que levanta el compose, o una variable.
fn host(m: &Manifest, target: &str, shard: u32) -> (String, String) {
    match target {
        "local" => (
            format!(
                "host = \"{}\"",
                crate::infra::nodo(&m.service, Some(m.pooler.shards.max(1)), shard)
            ),
            "port = 5432".into(),
        ),
        _ => (
            format!("host = \"${{AXON_DB_HOST_{shard}}}\""),
            format!("port = ${{AXON_DB_PORT_{shard}}}"),
        ),
    }
}

/// `users.toml`: pgdog no deja entrar a nadie que no este declarado aca.
///
/// La contrasena sale por variable salvo en local, donde el motor que levanta
/// el compose tiene una fija y conocida. Un secreto que no protege nada no
/// gana nada por estar en una variable, y perderia que el archivo generado
/// arranque solo.
pub fn users(ms: &[Manifest], target: &str, solo: Option<&str>) -> Result<String, String> {
    let m = elegir(ms, solo)?;
    let svc = &m.service;
    let clave = match target {
        "local" => "password = \"local\"".to_string(),
        _ => format!(
            "password = \"${{AXON_DB_PASSWORD_{}}}\"",
            svc.to_uppercase().replace('-', "_")
        ),
    };
    Ok(format!(
        "# generado por axon — no editar.\n\
         #   axon pooler manifests/ --service {svc} --users --target {target} > users.toml\n\
         \n\
         [[users]]\n\
         name = \"postgres\"\n\
         database = \"{base}\"\n\
         {clave}\n\
         # La misma regla que en pgdog.toml, otra vez aca: el ajuste por usuario\n\
         # gana sobre el general, asi que declararlo de un solo lado deja la\n\
         # puerta abierta por el otro.\n\
         cross_shard_disabled = {cross}\n",
        base = base(svc),
        cross = m.pooler.cross_shard_disabled
    ))
}

pub fn build(ms: &[Manifest], target: &str, solo: Option<&str>) -> Result<String, String> {
    let m = elegir(ms, solo)?;
    let esquemas = schemas(ms);
    let pl = &m.pooler;
    let svc = &m.service;
    let mut o = vec![
        "# generado por axon — no editar.".to_string(),
        format!("#   axon pooler manifests/ --service {svc} --target {target} > pgdog.toml"),
        "#".to_string(),
        match target {
            "local" => "# Los hosts son los contenedores que levanta `axon infra --target local`.",
            _ => "# Los nodos salen de variables: un archivo generado no es lugar\n\
                  # para una contrasena ni para la topologia de un entorno.",
        }
        .to_string(),
        String::new(),
        "[general]".into(),
        format!("pooler_mode = \"{}\"", pl.mode),
    ];
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
        // El parser tiene que estar SIEMPRE encendido: en `auto` no se activa
        // con un solo nodo primario, y ahi es justo donde una GUC de sesion se
        // cuela sin ser interceptada.
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

    // Un bloque `databases` por nodo.
    for shard in 0..pl.shards.max(1) {
        let (h, p) = host(m, target, shard);
        o.push("[[databases]]".into());
        o.push(format!("name = \"{}\"", base(svc)));
        o.push(h);
        o.push(p);
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
    // El compose local no levanta replicas: declararlas apuntando a un host
    // que no existe deja al pooler reintentando contra la nada.
    let replicas = match target {
        "local" => 0,
        _ => m.infra.read_replicas.unwrap_or(0),
    };
    for r in 0..replicas {
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
        let tablas = esquemas
            .get(svc)
            .ok_or_else(|| format!("{svc}: sin migraciones, no se puede declarar el reparto"))?;
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
            // el mismo hash que `PARTITION BY HASH` de Postgres, para que el
            // reparto coincida si algun dia se mueve dentro del motor
            o.push("hasher = \"postgres\"".into());
            o.push(String::new());
            declaradas += 1;
        }
        if declaradas == 0 {
            return Err(format!(
                "{svc}: ninguna tabla lleva `{clave}`, asi que no hay nada que repartir"
            ));
        }
        // pgdog rutea por la columna de inquilino cuando la reconoce en la
        // consulta. Es la misma columna que sostiene la RLS generada, y
        // declararla aca es lo que hace que un inquilino viva en un nodo.
        if let Some(col) = &m.infra.tenant_column {
            o.push("[multi_tenant]".into());
            o.push(format!("column = \"{col}\""));
            o.push(String::new());
        }
    }
    Ok(o.join("\n"))
}

//! MySQL E2E: spin up the official MySQL image via testcontainers,
//! apply the shared patchsets, verify state.
//!
//! Requires a running Docker daemon. Without it, `Mysql::default().start()`
//! errors and the tests fail loudly.

use diesel::Connection;
use diesel::mysql::MysqlConnection;
use testcontainers_modules::mysql;
use testcontainers_modules::testcontainers::runners::SyncRunner;

// MySQL identifiers: `LONGBLOB` for large binary, `DOUBLE` for floats, `TEXT`
// for variable text. `name` uses `VARCHAR(255)` because MySQL requires an
// explicit length on indexed text; the schema layout otherwise mirrors the
// SQLite and Postgres cases.
const USERS_DDL: &str = "\
CREATE TABLE users (
    id      BIGINT PRIMARY KEY,
    name    VARCHAR(255) NOT NULL,
    email   VARCHAR(255),
    score   DOUBLE
)";

const BLOBS_DDL: &str = "\
CREATE TABLE blobs (
    id      BIGINT PRIMARY KEY,
    payload LONGBLOB
)";

const KV_DDL: &str = "\
CREATE TABLE kv (
    tenant_id BIGINT NOT NULL,
    user_id   BIGINT NOT NULL,
    value     TEXT,
    PRIMARY KEY (tenant_id, user_id)
)";

fn boot() -> (
    testcontainers_modules::testcontainers::Container<mysql::Mysql>,
    MysqlConnection,
) {
    let container = mysql::Mysql::default()
        .start()
        .expect("start MySQL container (Docker must be running)");
    // `libmysqlclient` treats the host string `localhost` as a hint to use
    // the Unix socket. Normalize any loopback name to `127.0.0.1` so we
    // always connect over TCP to the container.
    let host = container.get_host().expect("container host").to_string();
    let host = if host == "localhost" {
        "127.0.0.1".into()
    } else {
        host
    };
    let port = container
        .get_host_port_ipv4(3306)
        .expect("container MySQL port");
    let url = format!("mysql://root@{host}:{port}/test");
    let conn = MysqlConnection::establish(&url).expect("connect to MySQL");
    (container, conn)
}

#[test]
fn users_insert_update_delete_full_cycle() {
    let (_container, mut conn) = boot();
    diesel_e2e::users_full_cycle(&mut conn, USERS_DDL);
}

#[test]
fn blobs_null_and_raw_bytes_roundtrip() {
    let (_container, mut conn) = boot();
    diesel_e2e::blobs_roundtrip(&mut conn, BLOBS_DDL);
}

#[test]
fn kv_composite_pk_lifecycle() {
    let (_container, mut conn) = boot();
    diesel_e2e::kv_lifecycle(&mut conn, KV_DDL);
}

//! MySQL async E2E: spin up the official MySQL image via testcontainers, then
//! drive the shared patchsets through `diesel-async` over a native
//! `AsyncMysqlConnection` and verify state.
//!
//! Requires a running Docker daemon. Without it, `Mysql::default().start()`
//! errors and the tests fail loudly.

use diesel_async::{AsyncConnection, AsyncMysqlConnection};
use testcontainers_modules::mysql;
use testcontainers_modules::testcontainers::ContainerAsync;
use testcontainers_modules::testcontainers::runners::AsyncRunner;

// MySQL needs an explicit length on indexed text, so `name`/`email` use
// `VARCHAR(255)`; otherwise the layout mirrors the SQLite and Postgres cases.
const USERS_DDL: &str = "\
CREATE TABLE users (
    id      BIGINT PRIMARY KEY,
    name    VARCHAR(255) NOT NULL,
    email   VARCHAR(255),
    score   DOUBLE
)";

const KV_DDL: &str = "\
CREATE TABLE kv (
    tenant_id BIGINT NOT NULL,
    user_id   BIGINT NOT NULL,
    value     TEXT,
    PRIMARY KEY (tenant_id, user_id)
)";

async fn boot() -> (ContainerAsync<mysql::Mysql>, AsyncMysqlConnection) {
    let container = mysql::Mysql::default()
        .start()
        .await
        .expect("start MySQL container (Docker must be running)");
    // `localhost` hints libmysqlclient toward a Unix socket; normalize any
    // loopback name to `127.0.0.1` so we always connect over TCP.
    let host = container
        .get_host()
        .await
        .expect("container host")
        .to_string();
    let host = if host == "localhost" {
        "127.0.0.1".to_string()
    } else {
        host
    };
    let port = container
        .get_host_port_ipv4(3306)
        .await
        .expect("container MySQL port");
    let url = format!("mysql://root@{host}:{port}/test");
    let conn = AsyncMysqlConnection::establish(&url)
        .await
        .expect("connect to MySQL");
    (container, conn)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn users_insert_update_delete_full_cycle_async() {
    let (_container, mut conn) = boot().await;
    diesel_e2e::users_full_cycle_async(&mut conn, USERS_DDL).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn kv_composite_pk_lifecycle_async() {
    let (_container, mut conn) = boot().await;
    diesel_e2e::kv_lifecycle_async(&mut conn, KV_DDL).await;
}

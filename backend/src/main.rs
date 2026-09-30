use literouter::{breaker_probe, build_router, build_state, db};

#[tokio::main]
async fn main() {
    let db_path = std::env::var("LITEROUTER_DB").unwrap_or_else(|_| "literouter.db".to_string());
    let pool = db::init_pool(&db_path).await;
    let state = build_state(pool).await;

    // background: keep the `logs` table bounded — relay traffic is high
    // volume and every row is an INSERT, so without this the DB grows
    // unbounded. retention window is 7 days; sweep runs once on startup
    // and every hour after.
    {
        let pool = state.pool.clone();
        tokio::spawn(async move {
            loop {
                match db::cleanup_old_logs(&pool, 7).await {
                    Ok(n) if n > 0 => println!("log cleanup: removed {} rows older than 7d", n),
                    Ok(_) => {}
                    Err(e) => eprintln!("log cleanup failed: {}", e),
                }
                tokio::time::sleep(std::time::Duration::from_secs(3600)).await;
            }
        });
    }

    // background: probe task — every `breaker_probe_interval_secs`, find
    // every (channel, model) whose cooldown has elapsed and send a
    // synthetic ping to the upstream. Recovery is owned by this loop and
    // is independent of user traffic.
    breaker_probe::spawn(state.clone());

    let app = build_router(state);

    // serve built frontend if a dist directory exists
    // (docker layout: /app/dist; local dev layout: ../frontend/dist)
    let dist = ["./dist", "../frontend/dist"]
        .iter()
        .map(std::path::Path::new)
        .find(|p| p.exists());
    let app = if let Some(dist) = dist {
        // SPA fallback: unknown paths serve index.html so frontend routes
        // like /channels survive a full page refresh
        let index = dist.join("index.html");
        app.fallback_service(
            tower_http::services::ServeDir::new(dist)
                .fallback(tower_http::services::ServeFile::new(index)),
        )
    } else {
        app
    };

    let port = std::env::var("PORT").unwrap_or_else(|_| "3000".to_string());
    let addr = format!("0.0.0.0:{}", port);
    println!("literouter listening on http://{}", addr);
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .expect("bind failed");
    // `into_make_service_with_connect_info` is what puts the peer address in
    // scope for handlers that take `ConnectInfo<SocketAddr>` — without it
    // axum rejects the extractor and the relay can't record a client IP when
    // there's no reverse proxy setting X-Forwarded-For.
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .await
    .unwrap();
}

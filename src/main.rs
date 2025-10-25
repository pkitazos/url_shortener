use std::{
    collections::HashMap,
    error::Error,
    sync::{Arc, Mutex},
};

use axum::{
    Router,
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
};
use sqlx::{FromRow, Pool, Sqlite, SqlitePool};

#[derive(Debug, Clone)]
struct AppCtx {
    pool: Pool<Sqlite>,
    stl_cache: Arc<Mutex<HashMap<String, String>>>, // coule probably do with better names
    lts_cache: Arc<Mutex<HashMap<String, String>>>,
}

impl AppCtx {
    fn new(pool: Pool<Sqlite>) -> AppCtx {
        AppCtx {
            stl_cache: Arc::new(Mutex::new(HashMap::new())),
            lts_cache: Arc::new(Mutex::new(HashMap::new())),
            pool,
        }
    }
}

#[derive(FromRow)]
struct URL {
    long_url: String,
    short_code: String,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let pool = SqlitePool::connect("sqlite:urlshortener.db").await?; // expects the file to already exist

    sqlx::migrate!("./migrations").run(&pool).await?;

    println!("created db");

    let app = Router::new()
        .route("/", get(root))
        .route("/shorten", post(shorten)) // passing the long url as a query param
        .route("/redirect/{short_code}", get(redirect))
        .with_state(AppCtx::new(pool));

    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await.unwrap();
    println!("listening on port 3000...\n");
    axum::serve(listener, app).await.unwrap();
    Ok(())
}

async fn root() -> impl IntoResponse {
    println!("/ GET <--");
    return (StatusCode::OK, "Hello, World!".to_string());
}

fn cool_shortener(long_url: &String) -> String {
    // for now we just trim the string
    // the actual shortening mechanic is not super important
    long_url[1..7].to_owned()
}

/// C -> S : shorten(long_url) ... S -> C : success(short_code)
async fn shorten(
    State(ctx): State<AppCtx>,
    Query(params): Query<HashMap<String, String>>,
) -> impl IntoResponse {
    if let Some(long_url) = params.get("q") {
        println!("/shorten POST <-- {}", long_url);

        {
            // acquire lock on lts
            let lts_cache = ctx.lts_cache.lock().unwrap();
            if let Some(short_code) = lts_cache.get(long_url) {
                println!("\tfound in cache");
                // already in cache, means already in db, can just return
                return (StatusCode::OK, short_code.to_owned());
            }
            // release lock on lts
        }

        println!("\tcache miss - new entry");
        let short_code = cool_shortener(&long_url);
        println!("\tshortened to: {}", short_code);

        let url = URL {
            long_url: long_url.clone(),
            short_code: short_code.to_owned(),
        };

        if let Ok(_) = store_entry(&url, &ctx.pool).await {
            println!("\tsaved to db");
            // already checked that it's not in the cache

            {
                // acquire lock on lts
                let mut lts_cache = ctx.lts_cache.lock().unwrap();
                lts_cache.insert(long_url.clone(), short_code.clone());
                println!("\tstoring in lts cache");
                // release lock on lts
            }
            {
                // acquire lock on stl
                let mut stl_cache = ctx.stl_cache.lock().unwrap();
                stl_cache.insert(short_code.clone(), long_url.clone());
                println!("\tstoring in stl cache");
                // release lock on stl
            }

            return (StatusCode::OK, short_code);
        } else {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Something went wrong on our end".to_owned(),
            );
        }
    }

    (StatusCode::BAD_REQUEST, "URL was not provided".to_owned())
}

/// C -> S : redirect(short_code) ...  S -> C : {
///     found(long_url),
///     not_found()
/// }
async fn redirect(State(ctx): State<AppCtx>, Path(short_code): Path<String>) -> impl IntoResponse {
    println!("/redirect GET <-- {}", short_code);
    {
        // acquire lock on stl
        let stl_cache = ctx.stl_cache.lock().unwrap();
        if let Some(long_url) = stl_cache.get(&short_code) {
            println!("\tfound in cache");
            return (StatusCode::OK, long_url.to_owned());
        }
        // release lock on stl
    }
    println!("\tcache miss - looking in db");

    if let Ok(Some(url)) = lookup_entry(&short_code, &ctx.pool).await {
        println!("\tfound in db");
        return (StatusCode::OK, url.long_url.to_owned());
    }

    println!("\tnot in db");
    (
        StatusCode::NOT_FOUND,
        "Short code not recognised".to_owned(),
    )
}

/// S -> D : store(URL) . D -> S : ok() . D -> S : ok() . end,
async fn store_entry(url: &URL, pool: &sqlx::SqlitePool) -> Result<(), sqlx::Error> {
    let long_url = &url.long_url;
    let short_code = &url.short_code;

    sqlx::query!(
        "INSERT INTO url (long_url, short_code) VALUES ($1, $2)",
        long_url,
        short_code
    )
    .execute(pool)
    .await?;

    Ok(())
}

/// S -> D : lookup(short_code) . D -> S : {
///     not_found()
///     ok(URL)
/// }
async fn lookup_entry(
    short_code: &String,
    pool: &sqlx::SqlitePool,
) -> Result<Option<URL>, sqlx::Error> {
    let res = sqlx::query_as!(URL, "SELECT * FROM url WHERE short_code = $1", short_code)
        .fetch_optional(pool)
        .await?;

    Ok(res)
}

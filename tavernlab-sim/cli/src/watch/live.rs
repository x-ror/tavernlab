//! The browser view: the same advice the terminal prints, on a page you can
//! leave open beside the client.
//!
//! A separate little server rather than a tab in `tavernsim serve`, because
//! the two run at different times and for different reasons -- the lab is
//! opened to work on a deck, the watcher runs while you play -- and making
//! one depend on the other would mean starting both to get either.
//!
//! Loopback only, the same rule the lab's server holds to: this reads your
//! game as it happens, and nothing that reads your game should be reachable
//! from outside the machine it runs on.

use std::net::TcpListener;
use std::sync::Mutex;

use crate::serve::http::{self, Request, Response};

use super::Advice;

/// The latest advice, as the JSON the page reads.
///
/// A global rather than a handle threaded through `run`, `live`, `follow` and
/// `report`: there is one watcher per process and one page looking at it, and
/// four signatures widened to carry a `Mutex` would say less about the design
/// than this comment does.
static LATEST: Mutex<Option<String>> = Mutex::new(None);

/// Hand the current advice to the page, if one is being served.
pub fn publish(advice: &Advice) {
    let json = tavernlab_json::to_string(|o| {
        o.obj(|o| {
            o.str_field("title", &advice.title);
            o.field("sections", |o| {
                o.arr(|a| {
                    for (heading, lines) in &advice.sections {
                        if lines.is_empty() {
                            continue;
                        }
                        a.item(|o| {
                            o.obj(|o| {
                                o.str_field("heading", heading);
                                o.field("lines", |o| {
                                    o.arr(|a| {
                                        for line in lines {
                                            a.str_item(line);
                                        }
                                    })
                                });
                            });
                        });
                    }
                })
            });
        })
    });
    if let Ok(mut slot) = LATEST.lock() {
        *slot = Some(json);
    }
}

const PAGE: &str = include_str!("live.html");

/// Start the view on `port`, on a thread of its own.
///
/// Returns the address it bound, so the caller can print a URL that is true
/// rather than the one it asked for.
pub fn start(port: u16) -> std::io::Result<String> {
    let listener = TcpListener::bind(("127.0.0.1", port))?;
    let addr = listener.local_addr()?;
    std::thread::Builder::new()
        .name("tavernlab-watch-view".into())
        .spawn(move || {
            let _ = http::serve(listener, |req: &Request| match req.path.as_str() {
                "/" => Response::text(200, "text/html; charset=utf-8", PAGE),
                "/advice.json" => match LATEST.lock().ok().and_then(|s| s.clone()) {
                    Some(json) => Response::json(200, json),
                    // Not an error: the watcher has simply not read a game
                    // yet, and the page says so rather than showing a stale
                    // board or an empty one.
                    None => Response::json(
                        200,
                        tavernlab_json::to_string(|o| {
                            o.obj(|o| {
                                o.str_field("title", "чекаю на гру");
                                o.field("sections", |o| o.arr(|_| {}));
                            })
                        }),
                    ),
                },
                _ => Response::error(404, "not found"),
            });
        })?;
    Ok(format!("http://{addr}/"))
}

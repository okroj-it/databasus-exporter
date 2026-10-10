mod api;

fn main() {
    let email = std::env::var("DATABASUS_EMAIL").unwrap_or_default();
    let password = std::env::var("DATABASUS_PASSWORD").unwrap_or_default();
    let url = std::env::var("DATABASUS_URL").unwrap_or_else(|_| "http://localhost:4005".into());
    let mut client = api::Client::new(&url, email, password);
    match client.workspaces() {
        Ok(ws) => {
            for w in ws {
                let dbs = client.databases(&w.id).map(|d| d.len());
                println!("{}: {:?} databases", w.name, dbs);
            }
        }
        Err(e) => eprintln!("error: {e}"),
    }
}

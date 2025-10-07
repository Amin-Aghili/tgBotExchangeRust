use std::env;
use std::time::Duration;

use dotenv::dotenv;
use num_format::{Locale, ToFormattedString};
use reqwest::Client;
use scraper::{Html, Selector};
use serde::Deserialize;
use tokio::time::sleep;

#[derive(Deserialize)]
struct BtcTurkRes {
    success: bool,
    data: Vec<BtcTurkItem>,
}

#[derive(Deserialize)]
struct BtcTurkItem {
    last: f64,
}

fn fmt_int(n: i64) -> String {
    n.to_formatted_string(&Locale::en)
}

fn round_up_to_i64(v: f64) -> i64 {
    v.ceil() as i64
}

async fn fetch_tgju_rate(client: &Client, url: &str) -> Result<i64, String> {
    let resp = client
        .get(url)
        .header(
            "User-Agent",
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) Chrome/128.0",
        )
        .send()
        .await
        .map_err(|e| format!("Request error for {}: {}", url, e))?;

    let body = resp
        .text()
        .await
        .map_err(|e| format!("Read body error for {}: {}", url, e))?;

    let doc = Html::parse_document(&body);
    // selector used in your python code
    let selector = Selector::parse(".top-mobile-block .block-last-change-percentage .price")
        .map_err(|e| format!("Selector parse error: {}", e))?;

    if let Some(elem) = doc.select(&selector).next() {
        let raw = elem.text().collect::<Vec<_>>().join("").trim().to_string();
        // temizle: ویرگول و فاصله‌ها رو حذف کنیم
        let clean = raw
            .replace(",", "")
            .replace(" ", "")
            .replace("\u{200c}", "");
        match clean.parse::<i64>() {
            Ok(v) => Ok(v),
            Err(e) => Err(format!("Parse int error for '{}' : {}", clean, e)),
        }
    } else {
        Err(format!("Selector not found on {}", url))
    }
}

async fn fetch_usdt_try(client: &Client, url: &str) -> Result<f64, String> {
    let resp = client
        .get(url)
        .send()
        .await
        .map_err(|e| format!("BTCTurk request error: {}", e))?;
    let txt = resp
        .text()
        .await
        .map_err(|e| format!("BTCTurk read body error: {}", e))?;

    let parsed: Result<BtcTurkRes, _> = serde_json::from_str(&txt);
    match parsed {
        Ok(obj) => {
            if obj.success && !obj.data.is_empty() {
                Ok(obj.data[0].last)
            } else {
                Err("BTCTurk responded with success=false or empty data".to_string())
            }
        }
        Err(e) => Err(format!("BTCTurk json parse error: {} / body: {}", e, txt)),
    }
}

async fn send_telegram_message(client: &Client, bot_token: &str, chat_id: &str, text: &str) {
    let url = format!("https://api.telegram.org/bot{}/sendMessage", bot_token);
    let params = [("chat_id", chat_id), ("text", text)];
    match client.post(&url).form(&params).send().await {
        Ok(resp) => {
            let status = resp.status();
            if status.is_success() {
                println!("✅ پیام به تلگرام ارسال شد");
            } else {
                // چون resp در اینجا move می‌شه، متن رو جدا می‌خونیم و فقط status قبلاً ذخیره شده
                match resp.text().await {
                    Ok(body) => println!("⚠️ تلگرام پاسخ غیرموفق داد: {} / body: {}", status, body),
                    Err(_) => println!("⚠️ تلگرام پاسخ غیرموفق داد: {}", status),
                }
            }
        }
        Err(e) => println!("❌ خطا در ارسال به تلگرام: {}", e),
    }
}

#[tokio::main]
async fn main() {
    dotenv().ok(); // load .env if exists

    let bot_token = env::var("BOT_TOKEN").expect("BOT_TOKEN env var not set");
    let chat_id = env::var("CHANNEL_ID").expect("CHANNEL_ID env var not set");

    let urls = vec![
        ("USD", "https://www.tgju.org/profile/price_dollar_rl"),
        ("EUR", "https://www.tgju.org/profile/price_eur"),
        ("AED", "https://www.tgju.org/profile/price_aed"),
        ("CNY", "https://www.tgju.org/profile/sana_sell_cny"),
    ];

    let btcturk_url = "https://api.btcturk.com/api/v2/ticker?pairSymbol=USDT_TRY";

    let client = Client::builder()
        .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) Chrome/128.0")
        .build()
        .expect("Failed to build client");

    println!("▶️ peybot_rust started. Updating every 60 seconds...");

    loop {
        // collect rates
        let mut rates: std::collections::HashMap<&str, i64> = std::collections::HashMap::new();

        for (name, url) in &urls {
            match fetch_tgju_rate(&client, url).await {
                Ok(v) => {
                    rates.insert(name, v);
                    println!("{} = {}", name, fmt_int(v));
                }
                Err(e) => {
                    println!("⚠️ دریافت {} ناموفق: {}", name, e);
                }
            }
        }

        // need USD at least
        if !rates.contains_key("USD") {
            println!("⚠️ نرخ دلار پیدا نشد — منتظر 60 ثانیه...");
            sleep(Duration::from_secs(60)).await;
            continue;
        }

        // btcturk
        let rate_tr = match fetch_usdt_try(&client, btcturk_url).await {
            Ok(v) => v,
            Err(e) => {
                println!("⚠️ خطا در دریافت USDT_TRY: {}", e);
                sleep(Duration::from_secs(60)).await;
                continue;
            }
        };

        // compute lira -> toman logic: (riyal / rate_tr / 10)
        let usd_riyal = *rates.get("USD").unwrap() as f64;
        let toman_per_lira = usd_riyal / rate_tr / 10.0;
        let toman_per_lira_i64 = round_up_to_i64(toman_per_lira);

        // build message (فارسی)
        // build message (فارسی)
        let mut text = String::from("📊 نرخ لحظه‌ای ارز (به تومان):\n\n");

        // همه نرخ‌ها رو از ریال به تومان تبدیل کن (تقسیم بر 10)
        if let Some(v) = rates.get("USD") {
            text.push_str(&format!("💵 دلار: {} تومان\n", fmt_int(v / 10)));
        }
        if let Some(v) = rates.get("EUR") {
            text.push_str(&format!("💶 یورو: {} تومان\n", fmt_int(v / 10)));
        }
        if let Some(v) = rates.get("AED") {
            text.push_str(&format!("🇦🇪 درهم: {} تومان\n", fmt_int(v / 10)));
        }
        if let Some(v) = rates.get("CNY") {
            text.push_str(&format!("🇨🇳 یوآن چین: {} تومان\n", fmt_int(v / 10)));
        }

        text.push_str(&format!(
            "\n🇹🇷 لیر ترکیه: {} تومان\n",
            fmt_int(toman_per_lira_i64)
        ));

        text.push_str("\n🔄 به‌روزرسانی هر ۱ دقیقه\n\n");
        text.push_str(&chat_id);

        // send
        send_telegram_message(&client, &bot_token, &chat_id, &text).await;

        // wait 60s
        sleep(Duration::from_secs(60)).await;
    }
}

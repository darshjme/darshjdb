// DarshJDB — created by Darshankumar Joshi (github.com/darshjme)
// ddb-cache-server :: dispatch — command router for the RESP3 protocol
// server. Every command listed in Slice 11 Part A, item 4 is handled
// here.

use std::sync::Arc;
use std::time::Duration;

use ddb_cache::DdbCache;

use crate::codec::RespFrame;

#[derive(Debug, Default)]
pub struct Session {
    pub authenticated: bool,
    pub resp3: bool,
    pub subscriptions: Vec<String>,
}

impl Session {
    pub fn new(auth_required: bool) -> Self {
        Self {
            authenticated: !auth_required,
            resp3: false,
            subscriptions: Vec::new(),
        }
    }
}

pub struct Dispatcher {
    pub cache: Arc<DdbCache>,
    pub password: Option<String>,
}

impl Dispatcher {
    pub fn new(cache: Arc<DdbCache>, password: Option<String>) -> Self {
        Self { cache, password }
    }

    pub async fn handle(&self, session: &mut Session, frame: RespFrame) -> RespFrame {
        let argv = match frame_to_argv(&frame) {
            Some(v) if !v.is_empty() => v,
            _ => return RespFrame::err("ERR empty or invalid command"),
        };
        let cmd = String::from_utf8_lossy(&argv[0]).to_ascii_uppercase();
        let rest: Vec<&[u8]> = argv.iter().skip(1).map(|v| v.as_slice()).collect();

        match cmd.as_str() {
            "AUTH" => return self.auth(session, &rest),
            "HELLO" => return self.hello(session, &rest),
            "PING" => return self.ping(&rest),
            "QUIT" => return RespFrame::ok(),
            _ => {}
        }

        if !session.authenticated {
            return RespFrame::err("NOAUTH Authentication required");
        }

        match cmd.as_str() {
            "GET" => self.get(&rest),
            "SET" => self.set(&rest),
            "DEL" => self.del(&rest),
            "EXISTS" => self.exists(&rest),
            "EXPIRE" => self.expire(&rest),
            "TTL" => self.ttl(&rest),
            "KEYS" => self.keys(&rest),
            "HSET" => self.hset(&rest),
            "HGET" => self.hget(&rest),
            "HGETALL" => self.hgetall(&rest),
            "HDEL" => self.hdel(&rest),
            "HLEN" => self.hlen(&rest),
            "LPUSH" => self.lpush(&rest),
            "RPUSH" => self.rpush(&rest),
            "LPOP" => self.lpop(&rest),
            "RPOP" => self.rpop(&rest),
            "LRANGE" => self.lrange(&rest),
            "ZADD" => self.zadd(&rest),
            "ZRANGE" => self.zrange(&rest),
            "ZRANGEBYSCORE" => self.zrangebyscore(&rest),
            "ZRANK" => self.zrank(&rest),
            "ZREM" => self.zrem(&rest),
            "ZSCORE" => self.zscore(&rest),
            "SUBSCRIBE" => self.subscribe(session, &rest),
            "UNSUBSCRIBE" => self.unsubscribe(session, &rest),
            "PUBLISH" => self.publish(&rest),
            "XADD" => self.xadd(&rest),
            "XREAD" => self.xread(&rest),
            "XRANGE" => self.xrange(&rest),
            "BFADD" => self.bfadd(&rest).await,
            "BFEXISTS" => self.bfexists(&rest).await,
            "PFADD" => self.pfadd(&rest).await,
            "PFCOUNT" => self.pfcount(&rest).await,
            "FLUSH" | "FLUSHALL" | "FLUSHDB" => {
                self.cache.flush();
                RespFrame::ok()
            }
            "INFO" => RespFrame::bulk(self.cache.info().into_bytes()),
            _ => RespFrame::err(format!("ERR unknown command '{cmd}'")),
        }
    }

    // ── AUTH / HELLO / PING ────────────────────────────────────────────

    fn auth(&self, session: &mut Session, args: &[&[u8]]) -> RespFrame {
        match &self.password {
            None => {
                session.authenticated = true;
                RespFrame::ok()
            }
            Some(expected) => {
                let pw = match args.len() {
                    1 => String::from_utf8_lossy(args[0]).to_string(),
                    2 => String::from_utf8_lossy(args[1]).to_string(),
                    _ => {
                        return RespFrame::err("ERR wrong number of arguments for 'auth'");
                    }
                };
                if pw == *expected {
                    session.authenticated = true;
                    RespFrame::ok()
                } else {
                    RespFrame::err("WRONGPASS invalid username-password pair")
                }
            }
        }
    }

    fn hello(&self, session: &mut Session, args: &[&[u8]]) -> RespFrame {
        let mut want_resp3 = session.resp3;
        if let Some(first) = args.first()
            && let Ok(ver) = std::str::from_utf8(first).unwrap_or("").parse::<i64>()
        {
            want_resp3 = ver >= 3;
        }
        session.resp3 = want_resp3;
        let pairs: Vec<(RespFrame, RespFrame)> = vec![
            (
                RespFrame::SimpleString("server".into()),
                RespFrame::bulk(b"ddb-cache-server".to_vec()),
            ),
            (
                RespFrame::SimpleString("version".into()),
                RespFrame::bulk(env!("CARGO_PKG_VERSION").as_bytes().to_vec()),
            ),
            (
                RespFrame::SimpleString("proto".into()),
                RespFrame::Integer(if want_resp3 { 3 } else { 2 }),
            ),
            (RespFrame::SimpleString("id".into()), RespFrame::Integer(0)),
            (
                RespFrame::SimpleString("mode".into()),
                RespFrame::bulk(b"standalone".to_vec()),
            ),
            (
                RespFrame::SimpleString("role".into()),
                RespFrame::bulk(b"master".to_vec()),
            ),
            (
                RespFrame::SimpleString("modules".into()),
                RespFrame::Array(Some(vec![])),
            ),
        ];
        RespFrame::Map(pairs)
    }

    fn ping(&self, args: &[&[u8]]) -> RespFrame {
        match args.first() {
            Some(payload) => RespFrame::bulk(payload.to_vec()),
            None => RespFrame::SimpleString("PONG".into()),
        }
    }

    // ── STRING ─────────────────────────────────────────────────────────

    fn get(&self, args: &[&[u8]]) -> RespFrame {
        let Some(key) = args.first().and_then(|a| std::str::from_utf8(a).ok()) else {
            return RespFrame::err("ERR wrong number of arguments for 'get'");
        };
        match self.cache.get(key) {
            Some(v) => RespFrame::bulk(v),
            None => RespFrame::nil_bulk(),
        }
    }

    fn set(&self, args: &[&[u8]]) -> RespFrame {
        if args.len() < 2 {
            return RespFrame::err("ERR wrong number of arguments for 'set'");
        }
        let Some(key) = std::str::from_utf8(args[0]).ok() else {
            return RespFrame::err("ERR invalid key");
        };
        let value = args[1].to_vec();
        let mut ttl: Option<Duration> = None;
        let mut i = 2;
        while i < args.len() {
            let opt = String::from_utf8_lossy(args[i]).to_ascii_uppercase();
            match opt.as_str() {
                "EX" => {
                    let Some(n) = args
                        .get(i + 1)
                        .and_then(|a| std::str::from_utf8(a).ok()?.parse::<u64>().ok())
                    else {
                        return RespFrame::err("ERR invalid EX value");
                    };
                    ttl = Some(Duration::from_secs(n));
                    i += 2;
                }
                "PX" => {
                    let Some(n) = args
                        .get(i + 1)
                        .and_then(|a| std::str::from_utf8(a).ok()?.parse::<u64>().ok())
                    else {
                        return RespFrame::err("ERR invalid PX value");
                    };
                    ttl = Some(Duration::from_millis(n));
                    i += 2;
                }
                _ => i += 1,
            }
        }
        self.cache.set(key.to_string(), value, ttl);
        RespFrame::ok()
    }

    fn del(&self, args: &[&[u8]]) -> RespFrame {
        let mut n = 0i64;
        for a in args {
            if let Ok(k) = std::str::from_utf8(a)
                && self.cache.del(k)
            {
                n += 1;
            }
        }
        RespFrame::Integer(n)
    }

    fn exists(&self, args: &[&[u8]]) -> RespFrame {
        let mut n = 0i64;
        for a in args {
            if let Ok(k) = std::str::from_utf8(a)
                && self.cache.exists(k)
            {
                n += 1;
            }
        }
        RespFrame::Integer(n)
    }

    fn expire(&self, args: &[&[u8]]) -> RespFrame {
        let Some(key) = args.first().and_then(|a| std::str::from_utf8(a).ok()) else {
            return RespFrame::err("ERR wrong number of arguments for 'expire'");
        };
        let Some(secs) = args
            .get(1)
            .and_then(|a| std::str::from_utf8(a).ok()?.parse::<u64>().ok())
        else {
            return RespFrame::err("ERR invalid expire value");
        };
        RespFrame::Integer(if self.cache.expire(key, Duration::from_secs(secs)) {
            1
        } else {
            0
        })
    }

    fn ttl(&self, args: &[&[u8]]) -> RespFrame {
        let Some(key) = args.first().and_then(|a| std::str::from_utf8(a).ok()) else {
            return RespFrame::err("ERR wrong number of arguments for 'ttl'");
        };
        RespFrame::Integer(self.cache.ttl(key))
    }

    fn keys(&self, args: &[&[u8]]) -> RespFrame {
        let pattern = args
            .first()
            .and_then(|a| std::str::from_utf8(a).ok())
            .unwrap_or("*");
        let items: Vec<RespFrame> = self
            .cache
            .keys(pattern)
            .into_iter()
            .map(|k| RespFrame::bulk(k.into_bytes()))
            .collect();
        RespFrame::Array(Some(items))
    }

    // ── HASH ───────────────────────────────────────────────────────────

    fn hset(&self, args: &[&[u8]]) -> RespFrame {
        if args.len() < 3 || !(args.len() - 1).is_multiple_of(2) {
            return RespFrame::err("ERR wrong number of arguments for 'hset'");
        }
        let Some(key) = std::str::from_utf8(args[0]).ok() else {
            return RespFrame::err("ERR invalid key");
        };
        let mut added = 0i64;
        let mut i = 1;
        while i + 1 < args.len() {
            if let Ok(field) = std::str::from_utf8(args[i])
                && self
                    .cache
                    .hset(key, field.to_string(), args[i + 1].to_vec())
            {
                added += 1;
            }
            i += 2;
        }
        RespFrame::Integer(added)
    }

    fn hget(&self, args: &[&[u8]]) -> RespFrame {
        if args.len() < 2 {
            return RespFrame::err("ERR wrong number of arguments for 'hget'");
        }
        let key = String::from_utf8_lossy(args[0]);
        let field = String::from_utf8_lossy(args[1]);
        match self.cache.hget(&key, &field) {
            Some(v) => RespFrame::bulk(v),
            None => RespFrame::nil_bulk(),
        }
    }

    fn hgetall(&self, args: &[&[u8]]) -> RespFrame {
        let Some(key) = args.first().and_then(|a| std::str::from_utf8(a).ok()) else {
            return RespFrame::err("ERR wrong number of arguments for 'hgetall'");
        };
        let pairs = self.cache.hgetall(key);
        let mut items = Vec::with_capacity(pairs.len() * 2);
        for (f, v) in pairs {
            items.push(RespFrame::bulk(f.into_bytes()));
            items.push(RespFrame::bulk(v));
        }
        RespFrame::Array(Some(items))
    }

    fn hdel(&self, args: &[&[u8]]) -> RespFrame {
        if args.len() < 2 {
            return RespFrame::err("ERR wrong number of arguments for 'hdel'");
        }
        let key = String::from_utf8_lossy(args[0]);
        let mut n = 0i64;
        for a in &args[1..] {
            if let Ok(f) = std::str::from_utf8(a)
                && self.cache.hdel(&key, f)
            {
                n += 1;
            }
        }
        RespFrame::Integer(n)
    }

    fn hlen(&self, args: &[&[u8]]) -> RespFrame {
        let Some(key) = args.first().and_then(|a| std::str::from_utf8(a).ok()) else {
            return RespFrame::err("ERR wrong number of arguments for 'hlen'");
        };
        RespFrame::Integer(self.cache.hlen(key) as i64)
    }

    // ── LIST ───────────────────────────────────────────────────────────

    fn lpush(&self, args: &[&[u8]]) -> RespFrame {
        self.push(args, true)
    }

    fn rpush(&self, args: &[&[u8]]) -> RespFrame {
        self.push(args, false)
    }

    fn push(&self, args: &[&[u8]], left: bool) -> RespFrame {
        if args.len() < 2 {
            return RespFrame::err("ERR wrong number of arguments for 'push'");
        }
        let Some(key) = std::str::from_utf8(args[0]).ok() else {
            return RespFrame::err("ERR invalid key");
        };
        let mut last_len = 0usize;
        for v in &args[1..] {
            last_len = if left {
                self.cache.lpush(key, v.to_vec())
            } else {
                self.cache.rpush(key, v.to_vec())
            };
        }
        RespFrame::Integer(last_len as i64)
    }

    fn lpop(&self, args: &[&[u8]]) -> RespFrame {
        let Some(key) = args.first().and_then(|a| std::str::from_utf8(a).ok()) else {
            return RespFrame::err("ERR wrong number of arguments for 'lpop'");
        };
        match self.cache.lpop(key) {
            Some(v) => RespFrame::bulk(v),
            None => RespFrame::nil_bulk(),
        }
    }

    fn rpop(&self, args: &[&[u8]]) -> RespFrame {
        let Some(key) = args.first().and_then(|a| std::str::from_utf8(a).ok()) else {
            return RespFrame::err("ERR wrong number of arguments for 'rpop'");
        };
        match self.cache.rpop(key) {
            Some(v) => RespFrame::bulk(v),
            None => RespFrame::nil_bulk(),
        }
    }

    fn lrange(&self, args: &[&[u8]]) -> RespFrame {
        if args.len() < 3 {
            return RespFrame::err("ERR wrong number of arguments for 'lrange'");
        }
        let Some(key) = std::str::from_utf8(args[0]).ok() else {
            return RespFrame::err("ERR invalid key");
        };
        let Some(start) = std::str::from_utf8(args[1])
            .ok()
            .and_then(|s| s.parse::<i64>().ok())
        else {
            return RespFrame::err("ERR invalid start");
        };
        let Some(stop) = std::str::from_utf8(args[2])
            .ok()
            .and_then(|s| s.parse::<i64>().ok())
        else {
            return RespFrame::err("ERR invalid stop");
        };
        let items: Vec<RespFrame> = self
            .cache
            .lrange(key, start, stop)
            .into_iter()
            .map(RespFrame::bulk)
            .collect();
        RespFrame::Array(Some(items))
    }

    // ── ZSET ───────────────────────────────────────────────────────────

    fn zadd(&self, args: &[&[u8]]) -> RespFrame {
        if args.len() < 3 || !(args.len() - 1).is_multiple_of(2) {
            return RespFrame::err("ERR wrong number of arguments for 'zadd'");
        }
        let Some(key) = std::str::from_utf8(args[0]).ok() else {
            return RespFrame::err("ERR invalid key");
        };
        let mut added = 0i64;
        let mut i = 1;
        while i + 1 < args.len() {
            let Some(score) = std::str::from_utf8(args[i])
                .ok()
                .and_then(|s| s.parse::<f64>().ok())
            else {
                return RespFrame::err("ERR invalid score");
            };
            let member = String::from_utf8_lossy(args[i + 1]).to_string();
            if self.cache.zadd(key, score, member) {
                added += 1;
            }
            i += 2;
        }
        RespFrame::Integer(added)
    }

    fn zrange(&self, args: &[&[u8]]) -> RespFrame {
        if args.len() < 3 {
            return RespFrame::err("ERR wrong number of arguments for 'zrange'");
        }
        let Some(key) = std::str::from_utf8(args[0]).ok() else {
            return RespFrame::err("ERR invalid key");
        };
        let Some(start) = std::str::from_utf8(args[1])
            .ok()
            .and_then(|s| s.parse::<i64>().ok())
        else {
            return RespFrame::err("ERR invalid start");
        };
        let Some(stop) = std::str::from_utf8(args[2])
            .ok()
            .and_then(|s| s.parse::<i64>().ok())
        else {
            return RespFrame::err("ERR invalid stop");
        };
        let with_scores = args.iter().any(|a| {
            std::str::from_utf8(a)
                .map(|s| s.eq_ignore_ascii_case("WITHSCORES"))
                .unwrap_or(false)
        });
        let r = self.cache.zrange(key, start, stop);
        let mut items = Vec::new();
        for (m, s) in r {
            items.push(RespFrame::bulk(m.into_bytes()));
            if with_scores {
                items.push(RespFrame::bulk(s.to_string().into_bytes()));
            }
        }
        RespFrame::Array(Some(items))
    }

    fn zrangebyscore(&self, args: &[&[u8]]) -> RespFrame {
        if args.len() < 3 {
            return RespFrame::err("ERR wrong number of arguments for 'zrangebyscore'");
        }
        let Some(key) = std::str::from_utf8(args[0]).ok() else {
            return RespFrame::err("ERR invalid key");
        };
        let Some(min) = std::str::from_utf8(args[1])
            .ok()
            .and_then(|s| s.parse::<f64>().ok())
        else {
            return RespFrame::err("ERR invalid min");
        };
        let Some(max) = std::str::from_utf8(args[2])
            .ok()
            .and_then(|s| s.parse::<f64>().ok())
        else {
            return RespFrame::err("ERR invalid max");
        };
        let r = self.cache.zrangebyscore(key, min, max);
        let items: Vec<RespFrame> = r
            .into_iter()
            .map(|(m, _)| RespFrame::bulk(m.into_bytes()))
            .collect();
        RespFrame::Array(Some(items))
    }

    fn zrank(&self, args: &[&[u8]]) -> RespFrame {
        if args.len() < 2 {
            return RespFrame::err("ERR wrong number of arguments for 'zrank'");
        }
        let key = String::from_utf8_lossy(args[0]);
        let member = String::from_utf8_lossy(args[1]);
        match self.cache.zrank(&key, &member) {
            Some(r) => RespFrame::Integer(r as i64),
            None => RespFrame::nil_bulk(),
        }
    }

    fn zrem(&self, args: &[&[u8]]) -> RespFrame {
        if args.len() < 2 {
            return RespFrame::err("ERR wrong number of arguments for 'zrem'");
        }
        let key = String::from_utf8_lossy(args[0]);
        let mut n = 0i64;
        for a in &args[1..] {
            if self.cache.zrem(&key, &String::from_utf8_lossy(a)) {
                n += 1;
            }
        }
        RespFrame::Integer(n)
    }

    fn zscore(&self, args: &[&[u8]]) -> RespFrame {
        if args.len() < 2 {
            return RespFrame::err("ERR wrong number of arguments for 'zscore'");
        }
        let key = String::from_utf8_lossy(args[0]);
        let member = String::from_utf8_lossy(args[1]);
        match self.cache.zscore(&key, &member) {
            Some(s) => RespFrame::bulk(s.to_string().into_bytes()),
            None => RespFrame::nil_bulk(),
        }
    }

    // ── PUB/SUB ────────────────────────────────────────────────────────

    fn subscribe(&self, session: &mut Session, args: &[&[u8]]) -> RespFrame {
        for a in args {
            if let Ok(ch) = std::str::from_utf8(a) {
                let _ = self.cache.subscribe(ch);
                session.subscriptions.push(ch.to_string());
            }
        }
        RespFrame::Array(Some(vec![
            RespFrame::bulk(b"subscribe".to_vec()),
            RespFrame::bulk(
                args.first()
                    .map(|a| a.to_vec())
                    .unwrap_or_else(|| b"".to_vec()),
            ),
            RespFrame::Integer(session.subscriptions.len() as i64),
        ]))
    }

    fn unsubscribe(&self, session: &mut Session, args: &[&[u8]]) -> RespFrame {
        let before = session.subscriptions.len();
        if args.is_empty() {
            session.subscriptions.clear();
        } else {
            for a in args {
                if let Ok(ch) = std::str::from_utf8(a) {
                    session.subscriptions.retain(|s| s != ch);
                }
            }
        }
        RespFrame::Array(Some(vec![
            RespFrame::bulk(b"unsubscribe".to_vec()),
            RespFrame::Integer((before - session.subscriptions.len()) as i64),
        ]))
    }

    fn publish(&self, args: &[&[u8]]) -> RespFrame {
        if args.len() < 2 {
            return RespFrame::err("ERR wrong number of arguments for 'publish'");
        }
        let Some(channel) = std::str::from_utf8(args[0]).ok() else {
            return RespFrame::err("ERR invalid channel");
        };
        let delivered = self.cache.publish(channel, args[1].to_vec());
        RespFrame::Integer(delivered as i64)
    }

    // ── STREAM ─────────────────────────────────────────────────────────

    fn xadd(&self, args: &[&[u8]]) -> RespFrame {
        if args.len() < 4 {
            return RespFrame::err("ERR wrong number of arguments for 'xadd'");
        }
        let Some(key) = std::str::from_utf8(args[0]).ok() else {
            return RespFrame::err("ERR invalid key");
        };
        let mut fields = Vec::new();
        let mut i = 2;
        while i + 1 < args.len() {
            let f = String::from_utf8_lossy(args[i]).to_string();
            let v = String::from_utf8_lossy(args[i + 1]).to_string();
            fields.push((f, v));
            i += 2;
        }
        let id = self.cache.xadd(key, fields);
        RespFrame::bulk(id.into_bytes())
    }

    fn xrange(&self, args: &[&[u8]]) -> RespFrame {
        if args.len() < 3 {
            return RespFrame::err("ERR wrong number of arguments for 'xrange'");
        }
        let Some(key) = std::str::from_utf8(args[0]).ok() else {
            return RespFrame::err("ERR invalid key");
        };
        let start = String::from_utf8_lossy(args[1]);
        let end = String::from_utf8_lossy(args[2]);
        let entries = self.cache.xrange(key, &start, &end);
        let items: Vec<RespFrame> = entries
            .into_iter()
            .map(|e| {
                let id = RespFrame::bulk(e.id.into_bytes());
                let mut field_items = Vec::new();
                for (f, v) in e.fields {
                    field_items.push(RespFrame::bulk(f.into_bytes()));
                    field_items.push(RespFrame::bulk(v.into_bytes()));
                }
                RespFrame::Array(Some(vec![id, RespFrame::Array(Some(field_items))]))
            })
            .collect();
        RespFrame::Array(Some(items))
    }

    fn xread(&self, args: &[&[u8]]) -> RespFrame {
        let mut iter = args.iter();
        let mut key: Option<String> = None;
        let mut id: Option<String> = None;
        while let Some(a) = iter.next() {
            let s = String::from_utf8_lossy(a).to_ascii_uppercase();
            if s == "STREAMS" {
                key = iter.next().map(|a| String::from_utf8_lossy(a).to_string());
                id = iter.next().map(|a| String::from_utf8_lossy(a).to_string());
                break;
            }
        }
        let Some(key) = key else {
            return RespFrame::err("ERR syntax error");
        };
        let id = id.unwrap_or_else(|| "0".to_string());
        let entries = self.cache.xread(&key, &id);
        let items: Vec<RespFrame> = entries
            .into_iter()
            .map(|e| {
                let id = RespFrame::bulk(e.id.into_bytes());
                let mut field_items = Vec::new();
                for (f, v) in e.fields {
                    field_items.push(RespFrame::bulk(f.into_bytes()));
                    field_items.push(RespFrame::bulk(v.into_bytes()));
                }
                RespFrame::Array(Some(vec![id, RespFrame::Array(Some(field_items))]))
            })
            .collect();
        RespFrame::Array(Some(items))
    }

    // ── PROBABILISTIC ──────────────────────────────────────────────────

    async fn bfadd(&self, args: &[&[u8]]) -> RespFrame {
        if args.len() < 2 {
            return RespFrame::err("ERR wrong number of arguments for 'bfadd'");
        }
        let key = String::from_utf8_lossy(args[0]).to_string();
        RespFrame::Integer(if self.cache.bfadd(&key, args[1]).await {
            1
        } else {
            0
        })
    }

    async fn bfexists(&self, args: &[&[u8]]) -> RespFrame {
        if args.len() < 2 {
            return RespFrame::err("ERR wrong number of arguments for 'bfexists'");
        }
        let key = String::from_utf8_lossy(args[0]).to_string();
        RespFrame::Integer(if self.cache.bfexists(&key, args[1]).await {
            1
        } else {
            0
        })
    }

    async fn pfadd(&self, args: &[&[u8]]) -> RespFrame {
        if args.len() < 2 {
            return RespFrame::err("ERR wrong number of arguments for 'pfadd'");
        }
        let key = String::from_utf8_lossy(args[0]).to_string();
        let mut changed = false;
        for a in &args[1..] {
            if self.cache.pfadd(&key, a).await {
                changed = true;
            }
        }
        RespFrame::Integer(if changed { 1 } else { 0 })
    }

    async fn pfcount(&self, args: &[&[u8]]) -> RespFrame {
        let Some(key) = args.first().and_then(|a| std::str::from_utf8(a).ok()) else {
            return RespFrame::err("ERR wrong number of arguments for 'pfcount'");
        };
        RespFrame::Integer(self.cache.pfcount(key).await as i64)
    }
}

fn frame_to_argv(frame: &RespFrame) -> Option<Vec<Vec<u8>>> {
    match frame {
        RespFrame::Array(Some(items)) => {
            let mut out = Vec::with_capacity(items.len());
            for item in items {
                match item {
                    RespFrame::BulkString(Some(b)) => out.push(b.clone()),
                    RespFrame::SimpleString(s) => out.push(s.as_bytes().to_vec()),
                    RespFrame::Integer(i) => out.push(i.to_string().into_bytes()),
                    _ => return None,
                }
            }
            Some(out)
        }
        _ => None,
    }
}

use criterion::{Criterion, black_box, criterion_group, criterion_main};
use ddb_server::query::darshql::Parser;

fn bench_simple_select(c: &mut Criterion) {
    let query = "SELECT * FROM users WHERE age > 18 LIMIT 10";
    c.bench_function("parse_simple_select", |b| {
        b.iter(|| Parser::parse(black_box(query)).unwrap());
    });
}

fn bench_complex_select(c: &mut Criterion) {
    let query = "\
        SELECT name, email, ->works_at->company.name AS company \
        FROM users \
        WHERE age > 18 AND status = 'active' OR role = 'admin' \
        ORDER BY name ASC \
        LIMIT 50 \
        START 10";
    c.bench_function("parse_complex_select", |b| {
        b.iter(|| Parser::parse(black_box(query)).unwrap());
    });
}

fn bench_relate(c: &mut Criterion) {
    let query = "RELATE user:darsh->works_at->company:knowai SET since = '2024', role = 'founder'";
    c.bench_function("parse_relate", |b| {
        b.iter(|| Parser::parse(black_box(query)).unwrap());
    });
}

fn bench_define_table(c: &mut Criterion) {
    let query = "DEFINE TABLE users SCHEMAFULL";
    c.bench_function("parse_define_table", |b| {
        b.iter(|| Parser::parse(black_box(query)).unwrap());
    });
}

fn bench_batch_10(c: &mut Criterion) {
    let queries = "\
        SELECT * FROM users WHERE age > 18; \
        SELECT name FROM posts ORDER BY created DESC LIMIT 5; \
        CREATE user:alice SET name = 'Alice', age = 30; \
        UPDATE users SET active = true WHERE last_login > '2024-01-01'; \
        DELETE users WHERE banned = true; \
        RELATE user:alice->follows->user:bob; \
        SELECT ->friends FROM user:darsh; \
        INSERT INTO logs (level, msg) VALUES ('info', 'startup'); \
        DEFINE TABLE sessions SCHEMAFULL; \
        INFO FOR DB";
    c.bench_function("parse_batch_10", |b| {
        b.iter(|| Parser::parse(black_box(queries)).unwrap());
    });
}

criterion_group!(
    benches,
    bench_simple_select,
    bench_complex_select,
    bench_relate,
    bench_define_table,
    bench_batch_10,
);
criterion_main!(benches);

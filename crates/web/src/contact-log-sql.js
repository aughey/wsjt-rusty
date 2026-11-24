// sql.js adapter only; schemas, revisions, records and ADIF live in wsjt-log.
export function contactSqlBatch(db, sql) { db.run(sql); }
export function contactSqlExecute(db, sql, parameters) { db.run(sql, JSON.parse(parameters)); }
export function contactSqlQuery(db, sql, parameters) {
    const statement = db.prepare(sql);
    try {
        statement.bind(JSON.parse(parameters));
        const values = [];
        while (statement.step()) values.push(statement.get());
        return JSON.stringify(values);
    } finally { statement.free(); }
}

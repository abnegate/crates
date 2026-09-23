use rusqlite::types::FromSql;
use rusqlite::types::FromSqlResult;
use rusqlite::types::ToSql;
use rusqlite::types::ToSqlOutput;
use rusqlite::types::ValueRef;

use crate::value::SecretValue;

impl ToSql for SecretValue {
    fn to_sql(&self) -> ::rusqlite::Result<ToSqlOutput<'_>> {
        Ok(ToSqlOutput::Borrowed(ValueRef::Text(
            self.expose().as_bytes(),
        )))
    }
}

impl FromSql for SecretValue {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        value.as_str().map(SecretValue::new)
    }
}

#[cfg(test)]
mod tests {
    use ::rusqlite::Connection;

    use super::*;

    fn connection() -> Connection {
        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute("CREATE TABLE credential (token TEXT NOT NULL)", ())
            .unwrap();
        connection
    }

    #[test]
    fn round_trips_through_a_text_column() {
        let connection = connection();
        let secret = SecretValue::new("sk-live-0123456789abcdef");

        connection
            .execute("INSERT INTO credential (token) VALUES (?1)", (&secret,))
            .unwrap();

        let stored: SecretValue = connection
            .query_row("SELECT token FROM credential", (), |row| row.get(0))
            .unwrap();
        assert_eq!(stored, secret);
    }

    #[test]
    fn is_written_as_the_plain_text_the_column_holds() {
        let connection = connection();
        connection
            .execute(
                "INSERT INTO credential (token) VALUES (?1)",
                (&SecretValue::new("sk-live-plain"),),
            )
            .unwrap();

        let stored: String = connection
            .query_row("SELECT token FROM credential", (), |row| row.get(0))
            .unwrap();
        assert_eq!(stored, "sk-live-plain");
    }

    #[test]
    fn rejects_a_column_that_is_not_text() {
        let connection = Connection::open_in_memory().unwrap();
        let read: Result<SecretValue, _> =
            connection.query_row("SELECT 42", (), |row| row.get::<_, SecretValue>(0));
        assert!(read.is_err());
    }
}

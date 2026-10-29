import pathlib
import sqlite3
import uuid

# SQL query mechanics only. These rows are not Auth or restorable-state fixtures.
schema = pathlib.Path(
    "crates/nessa-server/src/webhooks/infrastructure/outbox/schema.sql"
).read_text()
for count in (1, 64):
    connection = sqlite3.connect(":memory:")
    connection.executescript(schema)
    for index in range(count):
        registration = str(uuid.uuid4())
        credential = f"credential-{index}"
        connection.execute(
            "INSERT INTO registration_intents VALUES(?,?,?,'{}',"
            "'https://recipient.test/',zeroblob(32),1000)",
            (registration, "owner", registration),
        )
        connection.execute(
            "INSERT INTO registrations(id,credential,principal,organization,owner,"
            "issued_at,destination,mac_key,active) VALUES(?,?,?,'org','owner',1000,"
            "'https://recipient.test/',zeroblob(32),1)",
            (registration, credential, f"principal-{index}"),
        )
    query = "SELECT EXISTS(SELECT 1 FROM registrations WHERE credential=?1)"
    plan = connection.execute("EXPLAIN QUERY PLAN " + query, ("credential-0",)).fetchall()
    assert any("COVERING INDEX" in row[3] and "credential=?" in row[3] for row in plan), plan
    assert connection.execute(query, ("credential-0",)).fetchone() == (1,)
    assert connection.execute(query, ("Credential-0",)).fetchone() == (0,)
    assert connection.execute(query, ("other-credential",)).fetchone() == (0,)
    connection.execute(
        "UPDATE registrations SET active=0,stop='{}' WHERE credential='credential-0'"
    )
    assert connection.execute(query, ("credential-0",)).fetchone() == (1,)
    print(
        "query mechanics only; not a restorable/Auth fixture; live count", count,
        "plan", plan,
        "active/stopped exact-match and distinct/case controls passed",
    )

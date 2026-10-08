
      CREATE TABLE IF NOT EXISTS memory_spaces (
        id TEXT PRIMARY KEY,
        name TEXT NOT NULL,
        kind TEXT NOT NULL,
        root_path TEXT NOT NULL DEFAULT '',
        root_key TEXT NOT NULL DEFAULT '',
        created_at TEXT NOT NULL,
        updated_at TEXT NOT NULL
      );
      CREATE UNIQUE INDEX IF NOT EXISTS memory_spaces_root_key ON memory_spaces(root_key) WHERE root_key <> '';
      CREATE TABLE IF NOT EXISTS memories (
        id TEXT PRIMARY KEY,
        space_id TEXT NOT NULL REFERENCES memory_spaces(id) ON DELETE CASCADE,
        title TEXT NOT NULL,
        content TEXT NOT NULL,
        type TEXT NOT NULL,
        topic_key TEXT NOT NULL,
        identity_key TEXT NOT NULL,
        source_type TEXT NOT NULL,
        source_id TEXT NOT NULL DEFAULT '',
        source_path TEXT NOT NULL DEFAULT '',
        session_id TEXT NOT NULL DEFAULT '',
        cwd TEXT NOT NULL DEFAULT '',
        evidence TEXT NOT NULL DEFAULT '',
        source_timestamp TEXT NOT NULL DEFAULT '',
        importance REAL NOT NULL DEFAULT 0.5,
        authority INTEGER NOT NULL,
        status TEXT NOT NULL DEFAULT 'active',
        revision INTEGER NOT NULL DEFAULT 1,
        superseded_by TEXT NOT NULL DEFAULT '',
        superseded_at TEXT,
        verified_at TEXT,
        expires_at TEXT,
        semantic_text TEXT NOT NULL DEFAULT '',
        semantic_status TEXT NOT NULL DEFAULT 'pending',
        semantic_updated_at TEXT,
        access_count INTEGER NOT NULL DEFAULT 0,
        last_accessed_at TEXT,
        created_at TEXT NOT NULL,
        updated_at TEXT NOT NULL
      );
      CREATE INDEX IF NOT EXISTS memories_active_space ON memories(space_id, status, importance DESC, updated_at DESC);
      CREATE INDEX IF NOT EXISTS memories_active_identity ON memories(space_id, status, identity_key);
      CREATE INDEX IF NOT EXISTS memories_semantic_queue ON memories(status, semantic_status, updated_at);
      CREATE TABLE IF NOT EXISTS memory_links (
        id TEXT PRIMARY KEY,
        space_id TEXT NOT NULL REFERENCES memory_spaces(id) ON DELETE CASCADE,
        source_id TEXT NOT NULL REFERENCES memories(id) ON DELETE CASCADE,
        target_id TEXT NOT NULL REFERENCES memories(id) ON DELETE CASCADE,
        relation TEXT NOT NULL,
        weight REAL NOT NULL DEFAULT 0.5,
        created_at TEXT NOT NULL,
        UNIQUE(source_id, target_id, relation)
      );
      CREATE TABLE IF NOT EXISTS memory_candidates (
        id TEXT PRIMARY KEY,
        space_id TEXT NOT NULL REFERENCES memory_spaces(id) ON DELETE CASCADE,
        title TEXT NOT NULL,
        content TEXT NOT NULL,
        type TEXT NOT NULL,
        topic_key TEXT NOT NULL,
        identity_key TEXT NOT NULL,
        fingerprint TEXT NOT NULL,
        source_type TEXT NOT NULL,
        source_id TEXT NOT NULL DEFAULT '',
        session_id TEXT NOT NULL DEFAULT '',
        cwd TEXT NOT NULL DEFAULT '',
        importance REAL NOT NULL DEFAULT 0.5,
        evidence TEXT NOT NULL DEFAULT '',
        confidence REAL NOT NULL DEFAULT 0.5,
        source_timestamp TEXT NOT NULL DEFAULT '',
        expires_at TEXT NOT NULL,
        created_at TEXT NOT NULL,
        updated_at TEXT NOT NULL,
        UNIQUE(space_id, identity_key)
      );
      CREATE INDEX IF NOT EXISTS memory_candidates_created ON memory_candidates(created_at DESC);
      CREATE TABLE IF NOT EXISTS memory_tombstones (
        id TEXT PRIMARY KEY,
        entity_type TEXT NOT NULL,
        action TEXT NOT NULL,
        replacement_id TEXT NOT NULL DEFAULT '',
        reason_code TEXT NOT NULL DEFAULT '',
        created_at TEXT NOT NULL,
        expires_at TEXT NOT NULL
      );
      PRAGMA user_version = 4;
    
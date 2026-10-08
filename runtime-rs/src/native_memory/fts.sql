
      CREATE VIRTUAL TABLE IF NOT EXISTS memory_fts USING fts5(title, content, semantic_text, content='memories', content_rowid='rowid');
      CREATE TRIGGER IF NOT EXISTS memories_ai AFTER INSERT ON memories BEGIN
        INSERT INTO memory_fts(rowid, title, content, semantic_text) VALUES (new.rowid, new.title, new.content, COALESCE(new.semantic_text, ''));
      END;
      CREATE TRIGGER IF NOT EXISTS memories_ad AFTER DELETE ON memories BEGIN
        INSERT INTO memory_fts(memory_fts, rowid, title, content, semantic_text) VALUES ('delete', old.rowid, old.title, old.content, COALESCE(old.semantic_text, ''));
      END;
      CREATE TRIGGER IF NOT EXISTS memories_au AFTER UPDATE OF title, content, semantic_text ON memories BEGIN
        INSERT INTO memory_fts(memory_fts, rowid, title, content, semantic_text) VALUES ('delete', old.rowid, old.title, old.content, COALESCE(old.semantic_text, ''));
        INSERT INTO memory_fts(rowid, title, content, semantic_text) VALUES (new.rowid, new.title, new.content, COALESCE(new.semantic_text, ''));
      END;
      INSERT INTO memory_fts(memory_fts) VALUES ('rebuild');
    
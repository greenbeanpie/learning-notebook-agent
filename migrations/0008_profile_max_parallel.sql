-- Per-profile limit on model requests in flight at once (parallel generation).
ALTER TABLE model_profiles ADD COLUMN max_parallel INTEGER NOT NULL DEFAULT 4;

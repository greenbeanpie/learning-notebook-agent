-- Provider preset the profile was created from (e.g. "mimo"); NULL = custom.
ALTER TABLE model_profiles ADD COLUMN preset TEXT;
-- Trade picture quality for capacity when sending media to the model.
ALTER TABLE model_profiles ADD COLUMN shrink_media INTEGER NOT NULL DEFAULT 0;

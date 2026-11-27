-- 0 means "no limit": output tokens are left to the provider, and requests are not capped per
-- profile (a provider that refuses with a rate limit makes requests wait instead).
-- Profiles still on the old defaults (8000/8192 output tokens, 4 parallel requests) move to
-- the new defaults; values the user chose deliberately are kept.
UPDATE model_profiles SET max_output_tokens = 0 WHERE max_output_tokens IN (8000, 8192);
UPDATE model_profiles SET max_parallel = 0 WHERE max_parallel = 4;

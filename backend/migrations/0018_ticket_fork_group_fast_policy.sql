-- 本 fork 已部署旧 main 的 0016、0017；保留其字节和编号，仅追加正式版的剩余变更。
-- 全局限制不能静默丢失。先在旧版将限制迁至分组，再关闭全局开关后升级。
DO $$
BEGIN
    IF EXISTS (SELECT 1 FROM runtime_settings WHERE disable_fast) THEN
        RAISE EXCEPTION 'Before upgrading the ticket fork, migrate the global Fast restriction to groups and turn off global disable_fast';
    END IF;
END
$$;

ALTER TABLE runtime_settings DROP COLUMN disable_fast;

/// Cluster connection-quota HASH: SET-absolute per node, reap fields whose
/// `node_stats` key is missing, optional over-quota revert.
///
/// KEYS[1] = `{prefix}:connections:{app_id}`
/// ARGV[1] = node_id
/// ARGV[2] = new_count (ignored when mode=0)
/// ARGV[3] = limit (`< 0` skips only the cap revert)
/// ARGV[4] = mode (`0` GET, `1` SET)
/// ARGV[5] = node_stats key prefix (`{prefix}:node_stats:`)
pub const QUOTA_LUA: &str = r#"
local hash = KEYS[1]
local node_id = ARGV[1]
local new_count = tonumber(ARGV[2])
local limit = tonumber(ARGV[3])
local mode = tonumber(ARGV[4])
local stats_prefix = ARGV[5]

local fields = redis.call('HKEYS', hash)
for _, field in ipairs(fields) do
  if field ~= node_id then
    if redis.call('EXISTS', stats_prefix .. field) == 0 then
      redis.call('HDEL', hash, field)
    end
  end
end

if mode == 1 then
  local old = redis.call('HGET', hash, node_id)
  redis.call('HSET', hash, node_id, new_count)
  local vals = redis.call('HVALS', hash)
  local sum = 0
  for _, v in ipairs(vals) do
    sum = sum + tonumber(v)
  end
  if limit >= 0 and sum > limit then
    if old then
      redis.call('HSET', hash, node_id, old)
    else
      redis.call('HDEL', hash, node_id)
    end
    return {0, sum}
  end
  return {1, sum}
end

local vals = redis.call('HVALS', hash)
local sum = 0
for _, v in ipairs(vals) do
  sum = sum + tonumber(v)
end
return {1, sum}
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quota_lua_is_set_absolute_not_hincr() {
        assert!(QUOTA_LUA.contains("HSET"));
        assert!(QUOTA_LUA.contains("HVALS"));
        assert!(QUOTA_LUA.contains("EXISTS"));
        assert!(QUOTA_LUA.contains("HDEL"));
        assert!(!QUOTA_LUA.contains("HINCRBY"));
        assert!(QUOTA_LUA.contains("limit >= 0"));
    }
}

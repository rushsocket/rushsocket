-- Each node owns one expiring roster per channel. The index expires when
-- the last roster stops refreshing. A roster stores JSON strings verbatim
-- so nested arrays, numeric IDs, and user_info survive a Lua round trip.
local index, owner = KEYS[1], KEYS[2]
local mode, ttl = ARGV[1], ARGV[2]
local function save(roster)
    if next(roster) then
        redis.call('SET', owner, cjson.encode(roster), 'EX', ttl)
        redis.call('SADD', index, owner)
        redis.call('EXPIRE', index, ttl)
    else
        redis.call('DEL', owner)
        redis.call('SREM', index, owner)
    end
end

local roster
if mode ~= 'read' then
    roster = cjson.decode(ARGV[3])
    save(roster)
    if mode == 'refresh' then return 1 end
end

local members = {}
local count = 0
for _, key in ipairs(redis.call('SMEMBERS', index)) do
    local value = redis.call('GET', key)
    if value then
        for id, info in pairs(cjson.decode(value)) do
            if not members[id] then count = count + 1 end
            members[id] = info
        end
    else
        redis.call('SREM', index, key)
    end
end

local user = ARGV[4]
if mode == 'remove' then return members[user] and 0 or 1 end
local is_new = 0
if mode == 'add' then
    if not members[user] then
        if count >= tonumber(ARGV[6]) then return {-1, {}} end
        is_new = 1
        members[user] = ARGV[5]
    end
    roster[user] = roster[user] or members[user]
    save(roster)
end
local result = {}
for id, info in pairs(members) do
    table.insert(result, id)
    table.insert(result, info)
end
return {is_new, result}

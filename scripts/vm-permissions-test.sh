#!/usr/bin/env bash
# Tests share-level access end to end in the test VM (docs/architecture.md,
# 4.4): Samba and zendiloosd enforce a share's rules, NFS
# names hosts, and on disk every share has one static ACL for the group
# zendiloos-files. With real clients inside the VM (smbclient, the NFS
# client, Docker) and the files read with stat and getfacl:
#
#   1. a pool; the accounts anna, ben and carl; the groups editors (may
#      write), with carl, and viewers (may read), with ben; a share
#      "projects" where anna writes and both groups have their rule, and
#      carl is denied from his own side (PUT /shares/access, answered with
#      204). Its folder is root:zendiloos-files, mode 2770, with the static
#      ACL and nobody named; Samba writes there as root
#   2. over SMB anna writes, ben reads but cannot write, carl (denied,
#      though in editors) and the administrator (no rule) cannot connect;
#      what anna writes takes the owner of the folder it lands in and the
#      group zendiloos-files
#   3. ben joins editors: his next connection writes; he leaves: his next
#      connection only reads. The groups' answers name no shares, and no
#      job walks the share's files
#   4. anna is disabled: SMB refuses her; enabled again, she writes; no job
#      walks the share's files
#   5. NFS by host: the VM's own address, not named, cannot mount; named
#      read-only, it reads and cannot write; 127.0.0.1, named read-write,
#      reads and writes whoever it claims to be, squashed to
#      zendiloos-files; a rule that asks for a squash is refused
#   6. an app's container in the group zendiloos-files reads and writes a
#      folder of the share that only the static ACL opens to the group, and
#      gets nowhere without the group (skipped without Docker in the VM)
#   7. a share as 0.2.0 left it: named entries on every folder and file for
#      anna, ben, carl, editors, viewers, zendiloos-users, the VM's own user
#      (UID 1000) and an app's account, a private recycle bin, the root
#      root:root 0770. Marked as not migrated in zendiloosd's database and
#      zendiloosd restarted, the job sharing.migrate_permissions gives the
#      root its owner, mode and static ACL, everything the group's entry
#      (folders the default entry too), takes the entries of the people and
#      their groups, leaves those of UID 1000 and the app, and keeps owners
#      and contents; SMB then works on it as on a new share
#   8. everything is deleted again
#
# Needs the VM running with the package installed and set up, NFS on in the
# file services; account and password as for vm-storage-test.sh. Installs
# the clients it uses (smbclient, the NFS client, sqlite3) in the VM if they
# are missing. Step 6 needs Docker in the VM and the image alpine:3.20 (or
# the network to fetch it). Uses two free disks of the VM and leaves pools
# in use alone; restarts zendiloosd once, and adds a stand-in app account
# (app-zpermtest) to the VM for a while.
set -euo pipefail

# The second VM unless told otherwise: the first holds a LAN test.
export ZENDILOOS_VM=${ZENDILOOS_VM:-2}

root=$(cd "$(dirname "$0")/.." && pwd)
base="https://127.0.0.1:${ZENDILOOS_HTTPS_PORT:-$((8442 + ${ZENDILOOS_VM:-1}))}"
api="$base/api/v1"
user=${ZENDILOOS_USER:-admin}
password=${ZENDILOOS_PASSWORD:-correct horse battery}
pool=permtest
# zendiloosd's database (its state_dir), and the account it runs as.
db=/var/lib/zendiloos/zendiloos.db
mnt=/mnt/permtest-nfs
app='app-zpermtest'
anna_pw='first secret phrase' ben_pw='second secret phrase' carl_pw='third secret phrase'
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

say() { printf '\033[1m%s\033[0m\n' "$*"; }
fail() { printf '\033[31mFAILED: %s\033[0m\n' "$*" >&2; exit 1; }
vm() { "$root/scripts/vm.sh" ssh "$@"; }

call() {
    local method=$1 path=$2 body=${3:-}
    local args=(-sk -b "$work/admin.jar" -c "$work/admin.jar" -X "$method" -H "Origin: $base"
                -H "X-CSRF-Token: $(cat "$work/admin.csrf" 2>/dev/null)")
    [[ -n $body ]] && args+=(-H 'Content-Type: application/json' -d "$body")
    curl "${args[@]}" "$api$path"
}

# The status code of a request, its body in $work/body.
status() {
    local method=$1 path=$2 body=${3:-}
    local args=(-sk -o "$work/body" -w '%{http_code}' -b "$work/admin.jar" -c "$work/admin.jar" -X "$method"
                -H "Origin: $base" -H "X-CSRF-Token: $(cat "$work/admin.csrf" 2>/dev/null)")
    [[ -n $body ]] && args+=(-H 'Content-Type: application/json' -d "$body")
    curl "${args[@]}" "$api$path"
}

sign_in() {
    curl -sk -c "$work/admin.jar" -H "Origin: $base" -H 'Content-Type: application/json' \
        -d "$(jq -n --arg name "$user" --arg password "$password" '{name: $name, password: $password}')" \
        "$api/auth/login" | jq -er .csrf_token >"$work/admin.csrf" || fail "cannot sign in as $user"
}

wait_for() {
    local what=$1 check=$2 seconds=${3:-180}
    for _ in $(seq 1 "$seconds"); do
        if eval "$check" >/dev/null 2>&1; then return 0; fi
        sleep 1
    done
    fail "timed out waiting for $what"
}

job_succeeded() {
    local id=$1 state
    [[ $id =~ ^[0-9]+$ ]] || fail "no job: $id"
    wait_for "job $id" "[[ \$(call GET /jobs/$id | jq -r .state) != running ]]" 600
    state=$(call GET "/jobs/$id" | jq -r '.state + " " + (.error_code // "") + " " + (.error_detail // "")')
    [[ $state == succeeded* ]] || fail "job $id: $state"
}

# The newest job's ID; 0 without any.
last_job() { call GET '/jobs?limit=1' | jq '.[0].id // 0'; }
# No job since job $1 worked on the files of share $2: no walk as before
# (sharing.apply_access), nor anything else named after the share.
no_walk() {
    local jobs
    jobs=$(call GET '/jobs?limit=200' | jq -c --argjson after "$1" --arg share "$2" \
        '[.[] | select(.id > $after and (.target == $share or .kind == "sharing.apply_access")) | {id, kind, target}]')
    [[ $jobs == '[]' ]] || fail "jobs worked on the files of $2: $jobs"
}

# getfacl's lines without what the mask leaves of them (`#effective:`).
acl_of() { vm "sudo getfacl -cpE '$1' 2>/dev/null"; }
# A line of the ACL that starts with $2.
has_entry() {
    local acl
    acl=$(acl_of "$1")
    grep -q "^$2" <<<"$acl" || fail "$1 lacks $2: $(tr '\n' ' ' <<<"$acl")"
}
lacks() {
    local acl
    acl=$(acl_of "$1")
    ! grep -q "^$2" <<<"$acl" || fail "$1 still has $2: $(tr '\n' ' ' <<<"$acl")"
}
owner_of() { vm "sudo stat -c '%U:%G' '$1'"; }
# A share's folder as every share has it: root:zendiloos-files, mode 2770,
# the group's entry, its default entry and a full mask.
root_folder() {
    local mode entry
    mode=$(vm "sudo stat -c '%U:%G %a' '$1'")
    [[ $mode == 'root:zendiloos-files 2770' ]] || fail "$1 is $mode"
    for entry in group:zendiloos-files:rwx default:group:zendiloos-files:rwx mask::rwx; do
        has_entry "$1" "$entry"
    done
}
# Nobody named in the ACL of $1 but the group zendiloos-files.
nobody_named() {
    local named
    named=$(acl_of "$1" | grep -E '^(default:)?(user|group):[^:]+:' | grep -vE '^(default:)?group:zendiloos-files:' || true)
    [[ -z $named ]] || fail "$1 names $(tr '\n' ' ' <<<"$named")"
}

smb() { # share account password command…
    local share=$1 account=$2 secret=$3; shift 3
    vm "cd /tmp/permtest && smbclient //localhost/$share -U '$account%$secret' -c '$*' 2>&1" || true
}
# Output is captured first: grep -q on a pipe would break the pipe.
smb_ok() { local out; out=$(smb "$@"); ! grep -q NT_STATUS <<<"$out" || fail "SMB as $2 on $1: $out"; }
smb_denied() { local out; out=$(smb "$@"); grep -q NT_STATUS_ACCESS_DENIED <<<"$out" || fail "SMB as $2 on $1 was allowed: $out"; }
smb_refused() { local out; out=$(smb "$@"); grep -q NT_STATUS <<<"$out" || fail "SMB as $2 on $1 connected: $out"; }
# Whether a new connection to share $1 as $2 (password $3) puts a new file.
smb_writes() { local out; out=$(smb "$1" "$2" "$3" "put probe.txt probe-$2-$RANDOM.txt"); ! grep -q NT_STATUS <<<"$out"; }

# Mounts share folder $2 from $1 at $mnt with the options $3; says why not.
mount_nfs() { vm "sudo mkdir -p $mnt && sudo timeout 60 mount -t nfs -o $3 $1:$2 $mnt 2>&1"; }

# Runs `sh -c $2` in alpine as UID 4242 in group $1, folder $3 as /data.
container() { vm "sudo docker run --rm --network none --user 4242:$1 -v $3:/data alpine:3.20 sh -c '$2' 2>&1"; }

rule() { jq -nc --arg kind "$1" --argjson id "$2" --arg access "$3" '{kind: $kind, id: $id, access: $access}'; }
rules() { jq -sc . <<<"$*"; }

say "Clients in the VM"
clients='test -x /usr/bin/smbclient && test -x /usr/sbin/mount.nfs && test -x /usr/bin/sqlite3'
vm "$clients || sudo DEBIAN_FRONTEND=noninteractive apt-get install -y -qq smbclient nfs-common sqlite3 >/dev/null"
# Without the client, every SMB check that expects a refusal would pass.
vm "$clients" || fail "smbclient, mount.nfs or sqlite3 is missing in the VM"
vm "mkdir -p /tmp/permtest && echo probe > /tmp/permtest/probe.txt && echo 'hello from anna' > /tmp/permtest/hello.txt"

say "Signing in as $user"
sign_in

say "Cleaning up an earlier run"
vm "sudo umount -l $mnt 2>/dev/null; true"
for name in projects legacy; do
    for id in $(call GET /shares | jq -r --arg name "$name" '.[] | select(.name == $name) | .id'); do
        job_succeeded "$(call POST "/shares/$id/delete" "{\"confirm\":\"$name\"}" | jq -r .id)"
    done
done
for name in editors viewers; do
    for id in $(call GET /groups | jq -r --arg name "$name" '.[] | select(.name == $name) | .id'); do
        call DELETE "/groups/$id" >/dev/null
    done
done
for name in anna ben carl; do
    for id in $(call GET /accounts | jq -r --arg name "$name" '.[] | select(.name == $name) | .id'); do
        call DELETE "/accounts/$id" >/dev/null
    done
done
vm "sudo userdel $app 2>/dev/null; sudo groupdel $app 2>/dev/null; true"
if call GET /storage/pools | jq -e --arg pool "$pool" '.[] | select(.name == $pool)' >/dev/null; then
    job_succeeded "$(call POST "/storage/pools/$pool/destroy" "{\"confirm\":\"$pool\"}" | jq -r .id)"
fi

say "1. A pool, three accounts, two groups and a share"
free=$(call GET /storage/disks | jq -c '[.[] | select((.usage == "unused" or .usage == "foreign") and .id_link != null and (.read_only | not)) | .name][:2]')
[[ $(jq length <<<"$free") == 2 ]] || fail "two free disks are needed, found $free"
plan=$(call POST /storage/pools/plan "{\"name\":\"$pool\",\"layout\":\"mirror\",\"disks\":$free}")
token=$(jq -er .token <<<"$plan") || fail "plan: $plan"
job_succeeded "$(call POST /storage/pools "{\"token\":\"$token\",\"confirm\":\"$pool\"}" | jq -r .id)"
account() { call POST /accounts "$(jq -nc --arg name "$1" --arg password "$2" '{name: $name, role: "user", password: $password}')" \
    | jq -er .id || fail "account $1"; }
anna=$(account anna "$anna_pw")
ben=$(account ben "$ben_pw")
carl=$(account carl "$carl_pw")
group() { call POST /groups "{\"name\":\"$1\",\"members\":$2}" | jq -er .id || fail "group $1"; }
editors=$(group editors "[$carl]")
viewers=$(group viewers "[$ben]")
share() { # name rules: the share's path, its id in $work/<name>.id
    local made
    made=$(call POST /shares "$(jq -nc --arg name "$1" --arg pool "$pool" --argjson access "$2" \
        '{name: $name, pool: $pool, access: $access}')")
    jq -er .id <<<"$made" >"$work/$1.id" || fail "share $1: $made"
    jq -er .path <<<"$made"
}
projects=$(share projects "$(rules "$(rule account "$anna" read_write)" "$(rule group "$viewers" read)" "$(rule group "$editors" read_write)")")
projects_id=$(cat "$work/projects.id")
[[ $(status PUT "/shares/access/account/$carl" "{\"rules\":[{\"share_id\":$projects_id,\"rule\":\"deny\"}]}") == 204 ]] \
    || fail "cannot deny carl: $(cat "$work/body")"
call GET "/shares/access/account/$carl" | jq -e --argjson id "$projects_id" '.[] | select(.share_id == $id) | .rule == "deny"' \
    >/dev/null || fail "carl is not denied: $(call GET "/shares/access/account/$carl")"
root_folder "$projects"
nobody_named "$projects"
[[ $(owner_of "$projects/.recycle") == root:zendiloos-files ]] || fail "the recycle bin: $(vm "sudo ls -la $projects")"
section=$(vm "sudo testparm -s --section-name=projects 2>/dev/null")
for line in 'force user = root' 'force group = zendiloos-files'; do
    grep -qxE "\s*$line" <<<"$section" || fail "Samba's section lacks '$line': $section"
done
grep -qE '^\s*invalid users = (.* )?carl( |$)' <<<"$section" || fail "carl is no invalid user: $section"
echo "   $projects: root:zendiloos-files 2770, the static ACL, nobody named"

say "2. SMB as writer, reader and denied"
# A folder anna owns, as the update leaves those made before it.
vm "sudo mkdir $projects/anna-folder && sudo chown anna $projects/anna-folder"
smb_ok projects anna "$anna_pw" 'put hello.txt; mkdir album; put hello.txt album/inner.txt; put hello.txt anna-folder/mine.txt'
smb_ok projects ben "$ben_pw" 'get hello.txt ben-copy.txt'
vm "grep -q 'hello from anna' /tmp/permtest/ben-copy.txt" || fail "ben read something else"
smb_denied projects ben "$ben_pw" 'put hello.txt ben.txt'
smb_refused projects carl "$carl_pw" 'ls'
smb_refused projects "$user" "$password" 'ls'
# What lands in a folder takes its owner, and the group zendiloos-files.
for made in hello.txt album album/inner.txt; do
    [[ $(owner_of "$projects/$made") == root:zendiloos-files ]] || fail "$made is $(owner_of "$projects/$made")"
done
[[ $(owner_of "$projects/anna-folder/mine.txt") == anna:zendiloos-files ]] \
    || fail "what landed in anna's folder is $(owner_of "$projects/anna-folder/mine.txt")"
has_entry "$projects/album" 'default:group:zendiloos-files:rwx'
has_entry "$projects/album/inner.txt" 'group:zendiloos-files:'
lacks "$projects/hello.txt" 'user:anna'
echo "   anna writes, ben reads, carl and $user are refused; new files take the folder's owner"

say "3. A group's new member writes on the next connection, without a walk"
since=$(last_job)
joined=$(call PATCH "/groups/$editors" "{\"members\":[$carl,$ben]}")
jq -e --argjson ben "$ben" '(.members | any(. == $ben)) and (has("pending_shares") | not)' <<<"$joined" >/dev/null \
    || fail "ben did not join editors: $joined"
# Samba reads the new rules within moments; each connection is checked anew.
wait_for "ben's next connection to write" "smb_writes projects ben \"\$ben_pw\"" 20
smb_ok projects ben "$ben_pw" 'put hello.txt from-ben.txt'
[[ $(owner_of "$projects/from-ben.txt") == root:zendiloos-files ]] || fail "ben's file is $(owner_of "$projects/from-ben.txt")"
# carl is in editors too, but his own denial wins.
smb_refused projects carl "$carl_pw" 'ls'
left=$(call PATCH "/groups/$editors" "{\"members\":[$carl]}")
jq -e --argjson ben "$ben" '(.members | any(. == $ben) | not) and (has("pending_shares") | not)' <<<"$left" >/dev/null \
    || fail "ben did not leave editors: $left"
wait_for "ben's next connection not to write" "! smb_writes projects ben \"\$ben_pw\"" 20
smb_ok projects ben "$ben_pw" 'get from-ben.txt'
smb_denied projects ben "$ben_pw" 'put hello.txt ben-again.txt'
no_walk "$since" projects
nobody_named "$projects"
echo "   joining and leaving reach the next connection; no job walked the files"

say "4. A disabled account is refused; enabled again, it writes, without a walk"
since=$(last_job)
disabled=$(call PATCH "/accounts/$anna" '{"disabled":true}')
jq -e '.disabled and (has("pending_shares") | not)' <<<"$disabled" >/dev/null || fail "disabling: $disabled"
wait_for "SMB to refuse anna" "! smb_writes projects anna \"\$anna_pw\"" 20
smb_refused projects anna "$anna_pw" 'ls'
enabled=$(call PATCH "/accounts/$anna" '{"disabled":false}')
jq -e '(.disabled | not) and (has("pending_shares") | not)' <<<"$enabled" >/dev/null || fail "enabling: $enabled"
wait_for "anna to write again" "smb_writes projects anna \"\$anna_pw\"" 20
no_walk "$since" projects
echo "   anna is refused while disabled and writes again once enabled"

say "5. NFS by host, everyone squashed to zendiloos-files"
call GET /file-services | jq -e .nfs >/dev/null || fail "NFS is off in the file services"
files_uid=$(vm "id -u zendiloos-files")
files_gid=$(vm "getent group zendiloos-files | cut -d: -f3")
[[ $files_uid =~ ^[0-9]+$ && $files_gid =~ ^[0-9]+$ ]] || fail "no user or group zendiloos-files: $files_uid $files_gid"
# The VM is its own NFS client, the only one there is: mounted through
# 127.0.0.1 the server sees 127.0.0.1, mounted through the VM's own address
# (the NAT card's, of its default route) it sees that address, as traffic
# to a local address keeps it as its source. The firewall lets both in
# (the loopback). Those mounts use NFSv3: an NFSv4 client may take a second
# address of a server it knows for the same server and go on through the
# first.
other=$(vm "ip -4 -o route get 1.1.1.1" | sed -n 's/.* src \([0-9.]*\).*/\1/p')
[[ $other =~ ^[0-9]+(\.[0-9]+){3}$ && $other != 127.* ]] || fail "the VM has no address of its own: $other"
nfs_rules() { call PATCH "/shares/$projects_id" "{\"nfs\":$1}"; }
answer=$(nfs_rules '[{"network":"127.0.0.1","write":true}]')
jq -e --argjson id "$projects_id" '.id == $id and (.nfs | length == 1) and (has("job") | not)' <<<"$answer" >/dev/null \
    || fail "the NFS rule: $answer"
exports=$(vm "sudo cat /etc/exports.d/zendiloos.exports")
grep -qF "$projects 127.0.0.1(rw,sync,no_subtree_check,all_squash,anonuid=$files_uid,anongid=$files_gid,sec=sys)" <<<"$exports" \
    || fail "the export: $exports"
active=$(vm "sudo exportfs -v")
grep -qF "$projects" <<<"$active" || fail "NFS does not export $projects: $active"
if out=$(mount_nfs "$other" "$projects" vers=3,nolock,retry=0); then
    vm "sudo umount $mnt"
    fail "$other, not named, mounted the share"
fi
echo "   $other, not named: $(tail -1 <<<"$out")"
nfs_rules "[{\"network\":\"127.0.0.1\",\"write\":true},{\"network\":\"$other\",\"write\":false}]" \
    | jq -e '.nfs | length == 2' >/dev/null || fail "cannot name $other"
out=$(mount_nfs "$other" "$projects" vers=3,nolock,retry=0) || fail "$other, named read-only, cannot mount: $out"
nfs=$(vm "sudo cat $mnt/hello.txt; (sudo touch $mnt/from-ro.txt 2>/dev/null && echo WRITABLE || true); sudo umount $mnt")
grep -q 'hello from anna' <<<"$nfs" || fail "$other, named read-only, does not read: $nfs"
! grep -q WRITABLE <<<"$nfs" || fail "$other, named read-only, wrote"
out=$(mount_nfs 127.0.0.1 "$projects" vers=4,retry=0) || fail "127.0.0.1 cannot mount: $out"
# Root and anna on the client alike: both are zendiloos-files on the NAS.
nfs=$(vm "sudo cat $mnt/hello.txt; (echo from root | sudo tee $mnt/nfs-root.txt >/dev/null && echo ROOT-WROTE || true); \
    (sudo -u anna sh -c 'echo from anna > $mnt/nfs-anna.txt' 2>/dev/null && echo ANNA-WROTE || true); sudo umount $mnt")
grep -q 'hello from anna' <<<"$nfs" || fail "127.0.0.1 does not read: $nfs"
grep -q ROOT-WROTE <<<"$nfs" || fail "root on 127.0.0.1 does not write: $nfs"
grep -q ANNA-WROTE <<<"$nfs" || fail "anna on 127.0.0.1 does not write: $nfs"
for made in nfs-root.txt nfs-anna.txt; do
    [[ $(owner_of "$projects/$made") == zendiloos-files:zendiloos-files ]] \
        || fail "$made, written over NFS, is $(owner_of "$projects/$made")"
done
smb_ok projects ben "$ben_pw" 'get nfs-anna.txt'
[[ $(status PATCH "/shares/$projects_id" '{"nfs":[{"network":"127.0.0.1","write":true,"squash":"none"}]}') == 422 ]] \
    || fail "a rule with a squash was taken: $(cat "$work/body")"
nfs_rules '[]' | jq -e '.nfs == []' >/dev/null || fail "cannot end the export"
echo "   named hosts read or write as zendiloos-files; $other, unnamed, could not mount"

say "6. An app's container in the group zendiloos-files reads and writes the share"
if ! vm "command -v docker >/dev/null && sudo docker info >/dev/null 2>&1"; then
    echo "   Docker is not set up in the VM; skipped"
elif ! vm "sudo docker image inspect alpine:3.20 >/dev/null 2>&1 || sudo docker pull -q alpine:3.20 >/dev/null 2>&1"; then
    echo "   no image alpine:3.20 in the VM and none to fetch; skipped"
else
    # A folder of root's group, so that only the group's named entry, inherited
    # from the share's static ACL, lets zendiloos-files in.
    data="$projects/app-data"
    vm "sudo mkdir $data && echo 'for the app' | sudo tee $data/given.txt >/dev/null && sudo chgrp root $data $data/given.txt"
    has_entry "$data" 'group:zendiloos-files:rwx'
    [[ $(vm "sudo stat -c '%a' $data") == *0 ]] || fail "others may enter $data"
    out=$(container "$files_gid" 'cat /data/given.txt && echo from the app > /data/app.txt && mkdir /data/made && echo deeper > /data/made/more.txt' "$data") \
        || fail "the app cannot write: $out"
    grep -q 'for the app' <<<"$out" || fail "the app read something else: $out"
    [[ $(vm "sudo stat -c '%u' $data/app.txt $data/made $data/made/more.txt" | sort -u) == 4242 ]] \
        || fail "what the app made is not its own: $(vm "sudo stat -c '%n %u:%g' $data/app.txt $data/made $data/made/more.txt")"
    [[ $(vm "sudo cat $data/made/more.txt") == deeper ]] || fail "the app's file holds something else"
    has_entry "$data/made" 'default:group:zendiloos-files:rwx'
    if out=$(container 4242 'touch /data/stranger.txt' "$data"); then
        fail "a container outside the group wrote the folder: $out"
    fi
    smb_ok projects anna "$anna_pw" 'get app-data/app.txt app.txt'
    vm "grep -q 'from the app' /tmp/permtest/app.txt" || fail "SMB shows anna something else"
    echo "   the app wrote as UID 4242 through the group's entry; without the group it could not"
fi

say "7. A share from 0.2.0 is migrated once, when zendiloosd starts"
legacy=$(share legacy "$(rules "$(rule account "$anna" read_write)" "$(rule group "$viewers" read)")")
# A stand-in for an app's account, as the App Center made them: app-<id>,
# its own group, the comment "zendiloOS app" and the next ID zendiloOS
# would hand out, so that no account's ID is taken.
ids=$(vm "sudo bash -s -- $app" <<'EOF'
set -e
app=$1
if ! getent passwd "$app" >/dev/null; then
    highest=$( { cut -d: -f3 /etc/passwd /etc/group; sed -n 's/^[ug]id=//p' /etc/zendiloos/id-high-water 2>/dev/null; } \
        | awk '$1 >= 2000 && $1 <= 59999' | sort -n | tail -n 1)
    next=$(( ${highest:-1999} + 1 ))
    groupadd --gid "$next" "$app"
    useradd --uid "$next" --gid "$next" --comment 'zendiloOS app' --home-dir /nonexistent --no-create-home \
        --shell /usr/sbin/nologin "$app"
fi
echo "$(id -u "$app") $(id -g "$app")"
EOF
)
read -r app_uid app_gid <<<"$ids"
[[ $app_uid =~ ^[0-9]+$ && $app_gid =~ ^[0-9]+$ ]] || fail "no app account: $ids"
users_gid=$(vm "getent group zendiloos-users | cut -d: -f3")
# Files as 0.2.0 left them: an entry for each rule (and denials), for UID
# 1000 and for the app on every folder and file, inherited in folders;
# nothing for zendiloos-files; no setgid; the owners who wrote them; the
# root and the bins' folder root's, anna's bin private.
vm "sudo bash -s -- '$legacy' $app" <<'EOF'
set -euo pipefail
cd "$1"
app=$2
mkdir -p docs/deep/a/b photos/2024 many .recycle/anna
echo 'report from anna' > docs/report.txt
echo 'deep down' > docs/deep/a/b/file.txt
printf '#!/bin/sh\necho run\n' > docs/run.sh
chmod 0755 docs/run.sh
echo beach > photos/2024/beach.jpg
echo 'the owner of the VM wrote this' > photos/foreign.txt
echo 'an app wrote this' > photos/app.db
seq -f 'many/f%g' 1 500 | xargs touch
echo 'deleted once' > .recycle/anna/old.txt
chown -R anna:zendiloos-users docs .recycle/anna
chown -R ben:zendiloos-users photos many docs/deep/a/b/file.txt
chown 1000:1000 photos/foreign.txt
chown "$app:$app" photos/app.db
find . -path ./.zfs -prune -o -type d -exec chmod g-s {} +
chown root:root . .recycle
chmod 00770 .
chmod 01770 .recycle
entries="u:anna:rwX,u:ben:rX,u:carl:---,g:editors:rwX,g:viewers:rX,g:zendiloos-users:---,u:1000:rwX,u:$app:rwX,g:$app:rX"
find . -path ./.zfs -prune -o ! -type l -print0 | xargs -0 setfacl -b
find . -path ./.zfs -prune -o ! -type l -print0 | xargs -0 setfacl -m "$entries"
find . -path ./.zfs -prune -o -type d -print0 | xargs -0 setfacl -d -m "${entries//X/x}"
setfacl -R -b .recycle/anna
chmod 0700 .recycle/anna
EOF
[[ $(vm "sudo stat -c '%U:%G %a' $legacy") == 'root:root 770' ]] || fail "the root is not as 0.2.0 left it"
has_entry "$legacy/docs/report.txt" 'user:anna:rw-'
lacks "$legacy/docs/report.txt" 'group:zendiloos-files'
# Every entry's owner and every file's checksum, below the root and the
# bins' folder (which the migration gives to root:zendiloos-files).
state_of() {
    vm "sudo bash -s -- '$1'" <<'EOF'
cd "$1"
find . -mindepth 1 \( -path ./.zfs -prune -o ! -path ./.recycle -printf '%p %U:%G\n' \) | sort
find . \( -path ./.zfs -prune -o -type f -print0 \) | sort -z | xargs -0 sha256sum
EOF
}
state_of "$legacy" >"$work/before"
# Not migrated yet, as every share after the update. zendiloosd is stopped
# while its database is changed, and started again in any case.
since=$(last_job)
vm "sudo systemctl stop zendiloosd"
marked=$(vm "sudo -u zendiloos sqlite3 -cmd '.timeout 10000' $db \
    \"UPDATE shares SET permissions_migrated_at = NULL WHERE name = 'legacy'; SELECT changes();\" 2>&1") || true
vm "sudo systemctl restart zendiloosd"
wait_for "zendiloosd" "curl -skf $api/health" 120
[[ $marked == 1 ]] || fail "cannot mark legacy as not migrated: $marked"
sign_in
migration() { call GET '/jobs?limit=200' | jq -c --arg share "$1" --argjson after "$2" \
    '[.[] | select(.kind == "sharing.migrate_permissions" and .target == $share and .id > $after)][0] // empty'; }
wait_for "the migration of legacy" "migration legacy $since | jq -e .id" 360
job_succeeded "$(migration legacy "$since" | jq -r .id)"
migrated=$(vm "sudo -u zendiloos sqlite3 -cmd '.timeout 10000' $db \
    \"SELECT permissions_migrated_at IS NOT NULL FROM shares WHERE name = 'legacy';\"")
[[ $migrated == 1 ]] || fail "legacy is not marked migrated: $migrated"
root_folder "$legacy"
[[ $(owner_of "$legacy/.recycle") == root:zendiloos-files ]] || fail "the bins' folder is $(owner_of "$legacy/.recycle")"
# What is wrong below and at the root, a line each: every folder and file
# needs the group's entry (rw, x only for programs), every folder its
# default entry too; no named entry of a managed ID (2000-59999, but the
# app's) or of zendiloos-users is left, access or default; outside the bins
# (private before), the entries of UID 1000 and of the app stay.
problems=$(vm "sudo bash -s -- '$legacy' $files_gid $users_gid $app_uid $app_gid" <<'EOF'
root=$1 files=$2 users=$3 app_uid=$4 app_gid=$5
while IFS= read -r -d '' entry; do
    acl=$(getfacl -cnpE -- "$entry" 2>/dev/null) || { echo "$entry: no ACL"; continue; }
    if [[ -d $entry ]]; then
        grep -qx "group:$files:rwx" <<<"$acl" || echo "$entry: no group:zendiloos-files:rwx"
        grep -qx "default:group:$files:rwx" <<<"$acl" || echo "$entry: no default:group:zendiloos-files:rwx"
    else
        grep -qE "^group:$files:rw[-x]\$" <<<"$acl" || echo "$entry: no group:zendiloos-files:rw"
    fi
    while IFS= read -r line; do
        [[ $line =~ ^(default:)?(user|group):([0-9]+): ]] || continue
        kind=${BASH_REMATCH[2]} number=${BASH_REMATCH[3]}
        managed=$(( number >= 2000 && number <= 59999 ))
        if [[ $kind == user ]] && (( managed && number != app_uid )); then
            echo "$entry: $line"
        elif [[ $kind == group ]] && (( (managed && number != app_gid) || number == users )); then
            echo "$entry: $line"
        fi
    done <<<"$acl"
    if [[ $entry != "$root/.recycle/"* ]]; then
        for kept in "user:1000:" "user:$app_uid:" "group:$app_gid:"; do
            grep -q "^$kept" <<<"$acl" || echo "$entry: lost $kept"
        done
    fi
done < <(find "$root" \( -path "$root/.zfs" -prune \) -o \( ! -type l -print0 \))
EOF
)
[[ -z $problems ]] || fail "after the migration ($(wc -l <<<"$problems") problems): $(head -20 <<<"$problems")"
has_entry "$legacy/docs/report.txt" 'group:zendiloos-files:rw-'
has_entry "$legacy/docs/run.sh" 'group:zendiloos-files:rwx'
has_entry "$legacy/docs/deep/a/b" 'default:group:zendiloos-files:rwx'
lacks "$legacy" 'user:anna'
lacks "$legacy/docs/report.txt" 'group:editors'
state_of "$legacy" >"$work/after"
diff "$work/before" "$work/after" >"$work/changed" || fail "owners or contents changed: $(head -20 "$work/changed")"
# Samba on the migrated share, as on a new one.
smb_ok legacy anna "$anna_pw" 'put hello.txt docs/after.txt'
[[ $(owner_of "$legacy/docs/after.txt") == anna:zendiloos-files ]] || fail "docs/after.txt is $(owner_of "$legacy/docs/after.txt")"
smb_ok legacy ben "$ben_pw" 'get docs/report.txt report.txt'
smb_denied legacy ben "$ben_pw" 'put hello.txt docs/ben.txt'
echo "   the root and everything below have the group's entries, nobody's own; owners and contents kept"

say "8. Cleaning up"
for name in projects legacy; do
    job_succeeded "$(call POST "/shares/$(cat "$work/$name.id")/delete" "{\"confirm\":\"$name\"}" | jq -r .id)"
done
for id in $editors $viewers; do call DELETE "/groups/$id" >/dev/null; done
for id in $anna $ben $carl; do call DELETE "/accounts/$id" >/dev/null; done
vm "sudo userdel $app 2>/dev/null; sudo groupdel $app 2>/dev/null; rm -rf /tmp/permtest; true"
job_succeeded "$(call POST "/storage/pools/$pool/destroy" "{\"confirm\":\"$pool\"}" | jq -r .id)"

say "Passed: SMB and NFS enforce the rules, group members and disabled accounts take effect without walks, apps reach the share through zendiloos-files, and a share from 0.2.0 is migrated with its files intact"

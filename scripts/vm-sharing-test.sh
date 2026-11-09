#!/usr/bin/env bash
# Tests shares end to end in the test VM, through the HTTPS API and with
# real clients inside the VM (smbclient, the NFS client):
#
#   1. a pool and two accounts, anna and ben, and a share "photos" where
#      anna may write and ben may only read
#   2. over SMB anna writes, ben reads and cannot write; others cannot
#      connect; the share's folder is root:zendiloos-files with the one
#      static ACL of that group, and what is written takes the owner of the
#      folder it lands in and the group zendiloos-files
#   3. the share is exported read-only over NFS to the VM itself, everyone
#      there squashed to zendiloos-files
#   4. the file manager, signed in as anna and ben, sees what SMB sees:
#      files from either side, owners, refusals; nobody else sees the
#      share, and neither `..` nor a symbolic link leads out of it
#   5. ben loses access: at once, without a job; SMB and the file manager
#      refuse him
#   6. deleting over SMB moves files to the share's recycle bin, a folder
#      for each person who deleted, which everyone with access sees;
#      turned to read only again, ben reads what he wrote but changes
#      nothing
#   7. the share and the pool are deleted again
#   8. Windows (WS-Discovery) and macOS (Bonjour) can find the NAS
#
# Before 7, a snapshot of the share shows up as a "Previous Version" over
# SMB and is deleted again.
#
# Needs the VM running with the package installed and set up; account and
# password as for vm-storage-test.sh. Installs the clients it uses
# (smbclient, avahi-browse) in the VM if they are missing. Uses two free disks of the VM and
# leaves pools in use alone.
set -euo pipefail

# The second VM unless told otherwise: the first holds a LAN test.
export ZENDILOOS_VM=${ZENDILOOS_VM:-2}

root=$(cd "$(dirname "$0")/.." && pwd)
base="https://127.0.0.1:${ZENDILOOS_HTTPS_PORT:-$((8442 + ${ZENDILOOS_VM:-1}))}"
api="$base/api/v1"
user=${ZENDILOOS_USER:-admin}
password=${ZENDILOOS_PASSWORD:-correct horse battery}
pool=sharetest
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

say() { printf '\033[1m%s\033[0m\n' "$*"; }
fail() { printf '\033[31mFAILED: %s\033[0m\n' "$*" >&2; exit 1; }
vm() { "$root/scripts/vm.sh" ssh "$@"; }

# A request as one of the signed-in sessions: admin, anna or ben.
call_as() {
    local session=$1 method=$2 path=$3 body=${4:-}
    local args=(-sk -b "$work/$session.jar" -c "$work/$session.jar" -X "$method" -H "Origin: $base"
                -H "X-CSRF-Token: $(cat "$work/$session.csrf" 2>/dev/null)")
    [[ -n $body ]] && args+=(-H 'Content-Type: application/json' -d "$body")
    curl "${args[@]}" "$api$path"
}
call() { call_as admin "$@"; }

sign_in() { # session name password
    curl -sk -c "$work/$1.jar" -H "Origin: $base" -H 'Content-Type: application/json' \
        -d "$(jq -n --arg name "$2" --arg password "$3" '{name: $name, password: $password}')" \
        "$api/auth/login" | jq -er .csrf_token >"$work/$1.csrf" || fail "cannot sign in as $2"
}

# The status code of a request, its body in $work/body.
status_as() {
    local session=$1 method=$2 path=$3; shift 3
    curl -sk -o "$work/body" -w '%{http_code}' -b "$work/$session.jar" -X "$method" -H "Origin: $base" \
        -H "X-CSRF-Token: $(cat "$work/$session.csrf")" "$@" "$api$path"
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
    wait_for "job $id" "[[ \$(call GET /jobs/$id | jq -r .state) != running ]]"
    state=$(call GET "/jobs/$id" | jq -r '.state + " " + (.error_code // "") + " " + (.error_detail // "")')
    [[ $state == succeeded* ]] || fail "job $id: $state"
}

smb() { # account password command…
    local account=$1 secret=$2; shift 2
    vm "cd /tmp && smbclient //localhost/photos -U '$account%$secret' -c '$*' 2>&1" || true
}
# Output is captured first: grep -q on a pipe would break the pipe.
smb_ok() { local out; out=$(smb "$@"); ! grep -q NT_STATUS <<<"$out" || fail "SMB as $1: $out"; }
smb_denied() { local out; out=$(smb "$@"); grep -q NT_STATUS_ACCESS_DENIED <<<"$out" || fail "SMB as $1 was allowed: $out"; }
smb_refused() { local out; out=$(smb "$@"); grep -q NT_STATUS <<<"$out" || fail "SMB as $1 connected: $out"; }
smb_fails() { local out; out=$(smb "$@"); grep -q NT_STATUS <<<"$out" || fail "SMB as $1 did $3: $out"; }

owner_of() { vm "sudo stat -c '%U:%G' '$1'"; }
# What lands in a folder takes the folder's owner and the group zendiloos-files.
owned_as_parent() {
    local want
    want="$(vm "sudo stat -c '%U' '$(dirname "$1")'"):zendiloos-files"
    [[ $(owner_of "$1") == "$want" ]] || fail "$1 is $(owner_of "$1"), not $want"
}
# The newest job's ID; 0 without any.
last_job() { call GET '/jobs?limit=1' | jq '.[0].id // 0'; }
# No job since job $1 worked on the files of the share: rule changes apply
# at once, to the share as a whole.
no_walk() {
    local jobs
    jobs=$(call GET '/jobs?limit=200' | jq -c --argjson after "$1" \
        '[.[] | select(.id > $after and (.target == "photos" or .kind == "sharing.apply_access")) | {id, kind, target}]')
    [[ $jobs == '[]' ]] || fail "jobs worked on the files: $jobs"
}
# Each client of the export of $path as `exportfs -v` shows it,
# client(options), whether on the path's line or the next.
exported() { vm "sudo exportfs -v" | awk -v p="$path" '$1 == p { if (NF > 1) print $2; else { getline; print $1 } }'; }
# The options of client $1, between commas.
export_options() { printf ',%s,' "$(exported | grep -F "$1(" | sed 's/^[^(]*(//; s/)$//')"; }

say "Clients in the VM"
vm "command -v smbclient >/dev/null && command -v avahi-browse >/dev/null \
    || sudo DEBIAN_FRONTEND=noninteractive apt-get install -y -qq smbclient avahi-utils >/dev/null"
# Without the client, every SMB check would see no NT_STATUS and pass.
vm "command -v smbclient" >/dev/null || fail "smbclient is missing in the VM"

say "Signing in as $user"
sign_in admin "$user" "$password"

say "Cleaning up an earlier run"
for id in $(call GET /shares | jq -r '.[] | select(.name == "photos") | .id'); do
    job_succeeded "$(call POST "/shares/$id/delete" '{"confirm":"photos"}' | jq -r .id)"
done
if call GET /storage/pools | jq -e --arg pool "$pool" '.[] | select(.name == $pool)' >/dev/null; then
    job_succeeded "$(call POST "/storage/pools/$pool/destroy" "{\"confirm\":\"$pool\"}" | jq -r .id)"
fi
for name in anna ben; do
    id=$(call GET /accounts | jq -r --arg name "$name" '.[] | select(.name == $name) | .id')
    [[ -n $id ]] && call DELETE "/accounts/$id" >/dev/null
done

say "1. Pool, accounts and the share"
# Two disks that are free or hold only leftovers; pools in use stay alone.
free=$(call GET /storage/disks | jq -c '[.[] | select((.usage == "unused" or .usage == "foreign") and .id_link != null and (.read_only | not)) | .name][:2]')
[[ $(jq length <<<"$free") == 2 ]] || fail "two free disks are needed, found $free"
echo "   disks: $free"
plan=$(call POST /storage/pools/plan "{\"name\":\"$pool\",\"layout\":\"mirror\",\"disks\":$free}")
token=$(jq -er .token <<<"$plan") || fail "plan: $plan"
job_succeeded "$(call POST /storage/pools "{\"token\":\"$token\",\"confirm\":\"$pool\"}" | jq -r .id)"
anna=$(call POST /accounts '{"name":"anna","role":"user","password":"first secret phrase"}' | jq -er .id) || fail "anna"
ben=$(call POST /accounts '{"name":"ben","role":"user","password":"second secret phrase"}' | jq -er .id) || fail "ben"
share=$(call POST /shares "{\"name\":\"photos\",\"description\":\"Family photos, 100% ours\",\"pool\":\"$pool\",\"access\":[{\"kind\":\"account\",\"id\":$anna,\"access\":\"read_write\"},{\"kind\":\"account\",\"id\":$ben,\"access\":\"read\"}],\"nfs\":[{\"network\":\"127.0.0.1\",\"write\":false}]}")
share_id=$(jq -er .id <<<"$share") || fail "share: $share"
path=$(jq -r .path <<<"$share")
echo "   $path"
options=$(vm "findmnt -n -o OPTIONS $path")
[[ ,$options, == *,nosuid,* && ,$options, == *,nodev,* ]] || fail "$path is mounted with $options"

# Settings reach the dataset: quota (refquota), compression, record size.
settings='{"settings":{"quota_bytes":2147483648,"compression":"zstd","record_size_bytes":1048576}}'
[[ $(status_as admin PATCH "/shares/$share_id" -H 'Content-Type: application/json' -d "$settings") == 200 ]] \
    || fail "cannot change the share's settings: $(cat "$work/body")"
dataset="$pool/photos"
props=$(vm "sudo zfs get -H -p -o value refquota,compression,recordsize $dataset" | tr '\n' ' ')
[[ $props == "2147483648 zstd 1048576 " ]] || fail "the dataset has $props"
call GET /shares | jq -e '.[] | select(.name == "photos") | .used_bytes >= 0' >/dev/null || fail "no usage"
# Every share's folder is alike: root:zendiloos-files, 2770, the group's
# entry and its default entry, nobody named; its one recycle bin is made
# with it, the share's too.
[[ $(vm "sudo stat -c '%U:%G %a' $path") == 'root:zendiloos-files 2770' ]] \
    || fail "the share's folder is $(vm "sudo stat -c '%U:%G %a' $path")"
acl=$(vm "sudo getfacl -cpE $path")
for entry in group:zendiloos-files:rwx default:group:zendiloos-files:rwx; do
    grep -qx "$entry" <<<"$acl" || fail "the share's folder lacks $entry: $acl"
done
named=$(grep -E '^(default:)?(user|group):[^:]+:' <<<"$acl" | grep -v 'group:zendiloos-files:' || true)
[[ -z $named ]] || fail "the folder names somebody: $named"
[[ $(owner_of "$path/.recycle") == root:zendiloos-files ]] || fail "the recycle bin: $(vm "sudo ls -la $path")"

say "2. SMB with the share's permissions"
vm "echo 'hello from anna' > /tmp/hello.txt"
smb_ok anna 'first secret phrase' 'put hello.txt; mkdir album; put hello.txt album/inner.txt'
smb_ok ben 'second secret phrase' 'get hello.txt ben-copy.txt'
vm "grep -q 'hello from anna' /tmp/ben-copy.txt" || fail "ben read something else"
smb_denied ben 'second secret phrase' 'put hello.txt ben.txt'
smb_refused admin "$password" 'ls'
# The description is the comment, without what Samba would expand.
vm "testparm -s 2>/dev/null | grep -qx '\s*comment = Family photos, 100 ours'" || fail "the share has no comment"
# The rules are not in the files: Samba writes as root, and what it writes
# takes the owner of the folder it lands in and the group zendiloos-files.
acl=$(vm "sudo getfacl -cpE $path $path/hello.txt")
! grep -qE '^(default:)?(user|group):(anna|ben):' <<<"$acl" || fail "the files name anna or ben: $acl"
for made in hello.txt album album/inner.txt; do owned_as_parent "$path/$made"; done

say "3. NFS, read-only to the VM itself, everyone squashed to zendiloos-files"
# Whoever the client claims to be, root and anna alike, is zendiloos-files
# on the NAS: they read what the group may read, and write nothing here.
nfs=$(vm "sudo mkdir -p /mnt/nfstest && sudo mount -t nfs -o vers=4 127.0.0.1:$path /mnt/nfstest && sudo -u anna ls /mnt/nfstest; (sudo -u anna touch /mnt/nfstest/x 2>/dev/null && echo WRITABLE || true); (sudo touch /mnt/nfstest/y 2>/dev/null && echo WRITABLE || true); (sudo cat /mnt/nfstest/hello.txt 2>/dev/null || true); sudo umount /mnt/nfstest")
echo "$nfs" | sed 's/^/   /'
grep -q hello.txt <<<"$nfs" || fail "NFS shows no files"
! grep -q WRITABLE <<<"$nfs" || fail "the read-only export is writable"
grep -q 'hello from anna' <<<"$nfs" || fail "root, squashed to zendiloos-files, cannot read hello.txt"
files_uid=$(vm "id -u zendiloos-files")
files_gid=$(vm "getent group zendiloos-files | cut -d: -f3")
options=$(export_options 127.0.0.1)
for option in ro all_squash "anonuid=$files_uid" "anongid=$files_gid" sec=sys; do
    [[ $options == *",$option,"* ]] || fail "the export lacks $option: $options"
done

say "3b. On the NAS only: off the network, files and rules kept"
call PATCH "/shares/$share_id" '{"shared":false}' | jq -e '.shared == false and (has("job") | not)' >/dev/null \
    || fail "cannot stop sharing"
vm "! testparm -s 2>/dev/null | grep -qx '\[photos\]'" || fail "Samba still offers the folder"
vm "! sudo exportfs -v | grep -qF '$path'" || fail "NFS still exports the folder"
smb_refused anna 'first secret phrase' 'ls'
vm "sudo test -f $path/hello.txt" || fail "the files went with the sharing"
[[ $(call GET /files/volumes | jq -r --argjson id "$share_id" '[.[].folders[]? | select(.id == $id)][0].shared') == false ]] \
    || fail "the explorer does not see it unshared: $(call GET /files/volumes)"
call PATCH "/shares/$share_id" '{"shared":true}' | jq -e '.shared == true' >/dev/null || fail "cannot share again"
vm "testparm -s 2>/dev/null | grep -qx '\[photos\]'" || fail "Samba does not offer it again"
vm "sudo exportfs -v | grep -qF '$path'" || fail "NFS does not export it again"
call GET /shares | jq -e --argjson id "$share_id" --argjson ben "$ben" \
    '.[] | select(.id == $id) | .access | any(.kind == "account" and .id == $ben and .access == "read")' >/dev/null \
    || fail "the rules went with the sharing: $(call GET /shares)"
smb_ok anna 'first secret phrase' 'ls'
smb_ok ben 'second secret phrase' 'ls'

say "3c. NFS rules with their options"
# Every client squashed to zendiloos-files; there is no squash to choose.
rules='[{"network":"127.0.0.1","write":false},{"network":"*.example.com","write":true,"async":true,"insecure":true},{"network":"10.9.8.0/24","write":true}]'
call PATCH "/shares/$share_id" "{\"nfs\":$rules}" | jq -e '.nfs | length == 3' >/dev/null || fail "cannot set NFS rules"
squashed=(all_squash "anonuid=$files_uid" "anongid=$files_gid" sec=sys)
for client in '*.example.com' 10.9.8.0/24 127.0.0.1; do
    want=("${squashed[@]}")
    case $client in
        '*.example.com') want+=(rw async insecure) ;;
        10.9.8.0/24) want+=(rw sync) ;;
        127.0.0.1) want+=(ro sync) ;;
    esac
    options=$(export_options "$client")
    for option in "${want[@]}"; do
        [[ $options == *",$option,"* ]] || fail "the export to $client lacks $option: $options"
    done
    [[ $options != *,no_root_squash,* ]] || fail "the export to $client keeps root: $options"
done
[[ $(status_as admin PATCH "/shares/$share_id" -H 'Content-Type: application/json' -d '{"nfs":[{"network":"10.9.8.0/24","write":true,"squash":"none"}]}') == 422 ]] \
    || fail "a rule with a squash was taken: $(cat "$work/body")"
[[ $(status_as admin PATCH "/shares/$share_id" -H 'Content-Type: application/json' -d '{"nfs":[{"network":"bad host!","write":false}]}') == 422 ]] \
    || fail "a client that is no host was taken"
call PATCH "/shares/$share_id" '{"nfs":[{"network":"127.0.0.1","write":false}]}' >/dev/null

say "4. The file manager sees what SMB sees"
# New accounts choose their own password first; it is their SMB password too.
anna_pw='fresh secret phrase one' ben_pw='fresh secret phrase two'
sign_in anna anna 'first secret phrase'
sign_in ben ben 'second secret phrase'
call_as anna POST /auth/password "{\"current_password\":\"first secret phrase\",\"new_password\":\"$anna_pw\"}"
call_as ben POST /auth/password "{\"current_password\":\"second secret phrase\",\"new_password\":\"$ben_pw\"}"
files="/files/shares/$share_id"
[[ $(call_as anna GET /files/shares | jq -r '.[] | select(.name == "photos") | .access') == read_write ]] \
    || fail "anna does not see the share"
[[ $(call_as ben GET /files/shares | jq -r '.[] | select(.name == "photos") | .access') == read ]] \
    || fail "ben does not see the share"
# Shares of other tests or of the person using the VM may give the
# administrator access; this one does not.
! call GET /files/shares | jq -e '.[] | select(.name == "photos")' >/dev/null \
    || fail "the administrator sees a share without a rule"
[[ $(status_as admin GET "$files/folder") == 404 ]] || fail "the administrator can list the share"

echo 'written in the browser' >"$work/web.txt"
code=$(status_as anna POST "$files/upload?path=album&name=from-web.txt" \
    -H 'Content-Type: application/octet-stream' --data-binary "@$work/web.txt")
[[ $code == 201 ]] || fail "anna's upload: $code $(cat "$work/body")"
# Uploads, as what Samba writes, take the folder's owner and the group
# zendiloos-files, and the group's entry from the folder: in a folder anna
# owns from before, they are hers.
owned_as_parent "$path/album/from-web.txt"
acl=$(vm "sudo getfacl -cpE $path/album/from-web.txt")
grep -q '^group:zendiloos-files:rw' <<<"$acl" || fail "the upload did not inherit the group's entry: $acl"
vm "sudo mkdir $path/anna-own && sudo chown anna $path/anna-own"
code=$(status_as anna POST "$files/upload?path=anna-own&name=hers.txt" \
    -H 'Content-Type: application/octet-stream' --data-binary "@$work/web.txt")
[[ $code == 201 ]] || fail "anna's upload to her folder: $code $(cat "$work/body")"
[[ $(owner_of "$path/anna-own/hers.txt") == anna:zendiloos-files ]] || fail "the upload to anna's folder is $(owner_of "$path/anna-own/hers.txt")"
vm "sudo rm -r $path/anna-own"
smb_ok ben "$ben_pw" 'get album/from-web.txt from-web.txt'
vm "grep -q 'written in the browser' /tmp/from-web.txt" || fail "SMB shows ben something else"

listing=$(call_as ben GET "$files/folder")
jq -e '.entries | map(.name) | index("hello.txt") and index("album")' <<<"$listing" >/dev/null \
    || fail "ben's listing: $listing"
[[ $(jq .writable <<<"$listing") == false ]] || fail "ben's folder is writable: $listing"
[[ $(call_as ben GET "$files/download?path=hello.txt") == 'hello from anna' ]] || fail "ben's download"
# The search looks below the folder, as each may: case does not matter,
# links are not followed, bins and hidden names stay out unless asked for.
vm "sudo ln -sfn /etc $path/album/etc-link && sudo touch $path/.hidden-web-note"
for who in anna ben; do
    found=$(call_as "$who" GET "$files/search?path=&text=WEB")
    jq -e '[.entries[].path] | index("album/from-web.txt")' <<<"$found" >/dev/null || fail "$who's search: $found"
    jq -e '[.entries[].path] | map(select(startswith(".recycle") or startswith(".hidden") or test("^album/etc-link/"))) | length == 0' \
        <<<"$found" >/dev/null || fail "$who's search went too far: $found"
done
call_as anna GET "$files/search?path=&text=web&hidden=true" | jq -e '[.entries[].path] | index(".hidden-web-note")' >/dev/null \
    || fail "a hidden name is not found when asked for"
[[ $(call_as anna GET "$files/search?path=&text=passwd" | jq '.entries | length') == 0 ]] || fail "the search followed a link"
vm "sudo rm $path/album/etc-link $path/.hidden-web-note"
# Through nginx: a byte range resumes a download; text may be shown inline,
# sandboxed, HTML never.
headers=$(curl -sk -D - -o "$work/part" -b "$work/ben.jar" -H 'Range: bytes=6-' "$api$files/download?path=hello.txt")
grep -qi '^HTTP/[0-9.]* 206' <<<"$headers" || fail "no partial download: $headers"
grep -qi '^content-range: bytes 6-15/16' <<<"$headers" || fail "content range: $headers"
[[ $(cat "$work/part") == 'from anna' ]] || fail "the part: $(cat "$work/part")"
headers=$(curl -sk -D - -o /dev/null -b "$work/ben.jar" "$api$files/download?path=hello.txt&inline=true")
grep -qi '^content-type: text/plain' <<<"$headers" && grep -qi '^content-disposition: inline' <<<"$headers" \
    && grep -qi '^content-security-policy: sandbox' <<<"$headers" || fail "no sandboxed preview: $headers"
code=$(status_as ben POST "$files/upload?name=ben.txt" -H 'Content-Type: application/octet-stream' --data-binary "@$work/web.txt")
[[ $code == 403 ]] || fail "ben could upload: $code"
code=$(status_as ben POST "$files/delete" -H 'Content-Type: application/json' -d '{"paths":["hello.txt"]}')
[[ $code == 403 ]] || fail "ben could delete: $code"

vm "sudo ln -s /etc $path/etc-link && sudo ln -s .. $path/up"
for escape in 'folder?path=etc-link' 'download?path=etc-link/passwd' 'download?path=up/photos/hello.txt' 'folder?path=..'; do
    code=$(status_as anna GET "$files/$escape")
    [[ $code != 200 ]] || fail "$escape led out of the share: $(cat "$work/body")"
done
vm "sudo rm $path/etc-link $path/up"

# A folder as a tar archive: as long as announced, and GNU tar unpacks it.
headers=$(curl -sk -D - -o "$work/album.tar" -b "$work/anna.jar" "$api$files/archive?path=album")
length=$(grep -i '^content-length:' <<<"$headers" | tr -dc '0-9')
[[ -n $length && $(stat -c %s "$work/album.tar") == "$length" ]] || fail "the archive is not as long as announced: $headers"
mkdir -p "$work/unpacked" && tar -xf "$work/album.tar" -C "$work/unpacked" || fail "tar cannot unpack the archive"
[[ $(cat "$work/unpacked/album/from-web.txt") == 'written in the browser' ]] || fail "the archive holds something else"

job_succeeded "$(call_as anna POST "$files/transfer" '{"mode":"copy","paths":["album"],"target_share":'"$share_id"',"target_path":""}' | jq -r .id)"
vm "sudo test -f '$path/album (2)/from-web.txt'" || fail "the copy is missing"
owned_as_parent "$path/album (2)"
owned_as_parent "$path/album (2)/inner.txt"
call_as anna POST "$files/rename" '{"path":"album (2)","name":"copy"}' | jq -e '.path == "copy"' >/dev/null || fail "rename"
owner=$(owner_of "$path/copy/from-web.txt")
job_succeeded "$(call_as anna POST "$files/delete" '{"paths":["copy"]}' | jq -r .id)"
vm "sudo test -f $path/.recycle/anna/copy/from-web.txt" || fail "the deleted folder is not in anna's recycle bin"
restored=$(call_as anna POST "$files/restore" '{"paths":[".recycle/anna/copy"]}')
[[ $(jq -r '.paths[0]' <<<"$restored") == copy ]] || fail "restore: $restored"
vm "sudo test -f $path/copy/from-web.txt && sudo test ! -e $path/.recycle/anna/copy" || fail "the folder is not back"
[[ $(owner_of "$path/copy/from-web.txt") == "$owner" ]] || fail "the restored file changed owner: $(owner_of "$path/copy/from-web.txt")"
job_succeeded "$(call_as anna POST "$files/delete" '{"paths":["copy"]}' | jq -r .id)"
smb_ok anna "$anna_pw" 'ls album/from-web.txt'

# What a folder holds, counted as the person may see it.
facts=$(call_as anna GET "$files/facts?path=")
want_files=$(vm "sudo find $path -mindepth 1 -not -type d -not -path '*/.*' | wc -l")
want_folders=$(vm "sudo find $path -mindepth 1 -type d -not -path '*/.*' | wc -l")
jq -e --argjson f "$want_files" --argjson d "$want_folders" '.files == $f and .folders == $d and .stopped == null and .created != null' \
    <<<"$facts" >/dev/null || fail "the counts: $facts (find: $want_files files, $want_folders folders)"

say "5. ben loses access"
# At once and for the share as a whole: the answer is the share, and no
# job goes through its files.
since=$(last_job)
update=$(call PATCH "/shares/$share_id" "{\"access\":[{\"kind\":\"account\",\"id\":$anna,\"access\":\"read_write\"}]}")
jq -e --argjson id "$share_id" '.id == $id and (.access | length == 1) and (has("job") | not)' <<<"$update" >/dev/null \
    || fail "the change: $update"
smb_refused ben "$ben_pw" 'ls'
[[ $(status_as ben GET "$files/folder") == 404 ]] || fail "ben can still list the share"
no_walk "$since"

say "6. The share's recycle bin"
smb_ok anna "$anna_pw" 'del hello.txt'
vm "sudo test -f $path/.recycle/anna/hello.txt" || fail "the file is not in the recycle bin, in anna's folder"
# A second writer.
update=$(call PATCH "/shares/$share_id" "{\"access\":[{\"kind\":\"account\",\"id\":$anna,\"access\":\"read_write\"},{\"kind\":\"account\",\"id\":$ben,\"access\":\"read_write\"}]}")
jq -e '.access | length == 2' <<<"$update" >/dev/null || fail "the change: $update"
vm "echo 'from ben' > /tmp/ben.txt"
smb_ok ben "$ben_pw" 'put ben.txt; del ben.txt'
# Office lock files and temporary files do not fill the bins.
vm "echo lock > '/tmp/~\$report.docx'"
smb_ok ben "$ben_pw" 'put ~$report.docx; del ~$report.docx'
vm "sudo test ! -e '$path/.recycle/ben/~\$report.docx'" || fail "a lock file went to the recycle bin"
vm "sudo test -f $path/.recycle/ben/ben.txt" || fail "ben's file is not in the recycle bin, in his folder"
# The bin is the share's: everyone with access sees what anybody deleted.
out=$(smb ben "$ben_pw" 'ls .recycle/anna/*')
grep -q hello.txt <<<"$out" || fail "ben does not see what anna deleted: $out"

say "6b. Read only again: ben reads what he wrote, but changes nothing"
vm "echo 'ben wrote this' > /tmp/ben-own.txt"
smb_ok ben "$ben_pw" 'put ben-own.txt'
owned_as_parent "$path/ben-own.txt"
since=$(last_job)
update=$(call PATCH "/shares/$share_id" "{\"access\":[{\"kind\":\"account\",\"id\":$anna,\"access\":\"read_write\"},{\"kind\":\"account\",\"id\":$ben,\"access\":\"read\"}]}")
jq -e '(has("job") | not)' <<<"$update" >/dev/null || fail "the change: $update"
smb_ok ben "$ben_pw" 'get ben-own.txt ben-back.txt'
vm "grep -q 'ben wrote this' /tmp/ben-back.txt" || fail "ben read something else"
smb_denied ben "$ben_pw" 'put ben-own.txt'
smb_fails ben "$ben_pw" 'del ben-own.txt'
[[ $(status_as ben POST "$files/delete" -H 'Content-Type: application/json' -d '{"paths":["ben-own.txt"]}') == 403 ]] \
    || fail "ben, a reader, could delete his file in the file manager"
vm "sudo test -f $path/ben-own.txt" || fail "a reader's file went"
# Writing again, he deletes it, into the bin.
update=$(call PATCH "/shares/$share_id" "{\"access\":[{\"kind\":\"account\",\"id\":$anna,\"access\":\"read_write\"},{\"kind\":\"account\",\"id\":$ben,\"access\":\"read_write\"}]}")
jq -e '(has("job") | not)' <<<"$update" >/dev/null || fail "the change: $update"
smb_ok ben "$ben_pw" 'del ben-own.txt'
vm "sudo test -f $path/.recycle/ben/ben-own.txt" || fail "ben's deletion is not in the recycle bin"
no_walk "$since"

say "Snapshots are Previous Versions"
snapshot=$(call POST "/shares/$share_id/snapshots" '{"note":"from the test"}' | jq -er .label) || fail "no snapshot"
# zdl-2026-11-05_19.40.01 is offered as @GMT-2026.11.05-19.40.01.
gmt=$(sed -E 's/^zdl-([0-9]{4})-([0-9]{2})-([0-9]{2})_([0-9]{2})\.([0-9]{2})\.([0-9]{2})$/@GMT-\1.\2.\3-\4.\5.\6/' <<<"$snapshot")
out=$(smb anna "$anna_pw" 'allinfo album/from-web.txt')
grep -qF -- "$gmt" <<<"$out" || fail "the snapshot is no previous version ($gmt): $out"
[[ $(call GET "/shares/$share_id/snapshots" | jq -r '.snapshots[0].note') == "from the test" ]] || fail "the snapshot's note"
# The file manager looks into the snapshot as it was and copies back from it.
files="/files/shares/$share_id"
call_as anna GET "$files/snapshots" | jq -e --arg s "$snapshot" '.[] | select(.label == $s)' >/dev/null \
    || fail "anna does not see the snapshot"
job_succeeded "$(call_as anna POST "$files/delete" '{"paths":["album/from-web.txt"]}' | jq -r .id)"
call_as anna GET "$files/folder?path=album&snapshot=$snapshot" | jq -e '.entries | map(.name) | index("from-web.txt")' >/dev/null \
    || fail "the snapshot does not show the deleted file"
[[ $(call_as anna GET "$files/download?path=album/from-web.txt&snapshot=$snapshot") == 'written in the browser' ]] \
    || fail "download from the snapshot"
job_succeeded "$(call_as anna POST "$files/snapshots/$snapshot/restore" '{"paths":["album/from-web.txt"]}' | jq -r .id)"
[[ $(vm "sudo cat $path/album/from-web.txt") == 'written in the browser' ]] || fail "the file is not back"
owned_as_parent "$path/album/from-web.txt"
# Rolling back: what changed since is gone, so are later snapshots.
later=$(call POST "/shares/$share_id/snapshots" '{"note":"later"}' | jq -er .label) || fail "no later snapshot"
vm "sudo touch $path/after-the-snapshot.txt"
[[ $(status_as admin POST "/shares/$share_id/snapshots/$snapshot/rollback" -H 'Content-Type: application/json' -d '{"confirm":"photo"}') == 422 ]] \
    || fail "a rollback without the right name"
rolled=$(call POST "/shares/$share_id/snapshots/$snapshot/rollback" '{"confirm":"photos"}')
[[ $(jq -r '.destroyed | join(",")' <<<"$rolled") == "$later" ]] || fail "rollback: $rolled"
# A snapshot from after the share's migration brings back no old
# permissions: nothing to walk.
jq -e '.job == null' <<<"$rolled" >/dev/null || fail "the rollback started a job: $rolled"
vm "sudo test ! -e $path/after-the-snapshot.txt" || fail "the rollback kept a later file"
vm "! sudo zfs list -H -t snapshot -o name | grep -qF '@$later'" || fail "the later snapshot is still there"
[[ $(status_as admin POST "/shares/$share_id/snapshots/$snapshot/delete") == 204 ]] || fail "cannot delete the snapshot"
vm "! sudo zfs list -H -t snapshot -o name | grep -qF '@$snapshot'" || fail "the snapshot is still there"

say "7. Deleting the share and the pool"
job_succeeded "$(call POST "/shares/$share_id/delete" '{"confirm":"photos"}' | jq -r .id)"
vm "! findmnt -n $path" || fail "$path is still mounted"
job_succeeded "$(call POST "/storage/pools/$pool/destroy" "{\"confirm\":\"$pool\"}" | jq -r .id)"
for id in $anna $ben; do call DELETE "/accounts/$id" >/dev/null; done
say "8. Windows and macOS find the NAS"
vm "systemctl is-active --quiet wsdd2" || fail "wsdd2 (WS-Discovery for Windows) does not run"
vm "ss -Hlun | grep -q ':3702 '" || fail "nothing answers WS-Discovery on port 3702"
vm "systemctl is-active --quiet avahi-daemon" || fail "avahi-daemon (Bonjour for macOS) does not run"
# grep -c reads to the end: with -q, avahi-browse could get SIGPIPE after
# the match, and pipefail would count that as failure.
wait_for "Samba's Bonjour announcement" "vm 'avahi-browse -tpr _smb._tcp' | grep -c '^=.*;445;'" 30

say "Passed: SMB, NFS, file manager, the shares' folders and owners, permission changes, the recycle bin, discovery"

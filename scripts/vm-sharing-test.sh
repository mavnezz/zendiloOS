#!/usr/bin/env bash
# Tests shares end to end in the test VM, through the HTTPS API and with
# real clients inside the VM (smbclient, the NFS client):
#
#   1. a pool and two accounts, anna and ben, and a share "photos" where
#      anna may write and ben may only read
#   2. over SMB anna writes, ben reads and cannot write; others cannot
#      connect; the share's directory carries the matching ACLs
#   3. the share is exported read-only over NFS to the VM itself
#   4. the file manager, signed in as anna and ben, sees what SMB sees:
#      files from either side, owners, ACLs, refusals; nobody else sees
#      the share, and neither `..` nor a symbolic link leads out of it
#   5. ben loses access: the change reaches existing files in a job and
#      SMB refuses him
#   6. deleting over SMB moves files to each writer's own recycle bin
#   7. the share and the pool are deleted again
#
# Needs the VM running with the package installed and set up; account and
# password as for vm-storage-test.sh. Uses two free disks of the VM and
# leaves pools in use alone.
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
base="https://127.0.0.1:${ZENDILOOS_HTTPS_PORT:-8443}"
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
share=$(call POST /shares "{\"name\":\"photos\",\"pool\":\"$pool\",\"access\":[{\"kind\":\"account\",\"id\":$anna,\"access\":\"read_write\"},{\"kind\":\"account\",\"id\":$ben,\"access\":\"read\"}],\"nfs\":[{\"network\":\"127.0.0.1\",\"write\":false}]}")
share_id=$(jq -er .id <<<"$share") || fail "share: $share"
path=$(jq -r .path <<<"$share")
echo "   $path"
options=$(vm "findmnt -n -o OPTIONS $path")
[[ ,$options, == *,nosuid,* && ,$options, == *,nodev,* ]] || fail "$path is mounted with $options"

say "2. SMB with the share's permissions"
vm "echo 'hello from anna' > /tmp/hello.txt"
smb_ok anna 'first secret phrase' 'put hello.txt; mkdir album; put hello.txt album/inner.txt'
smb_ok ben 'second secret phrase' 'get hello.txt ben-copy.txt'
vm "grep -q 'hello from anna' /tmp/ben-copy.txt" || fail "ben read something else"
smb_denied ben 'second secret phrase' 'put hello.txt ben.txt'
smb_refused admin "$password" 'ls'
acl=$(vm "sudo getfacl -p $path")
grep -q '^user:anna:rwx' <<<"$acl" && grep -q '^user:ben:r-x' <<<"$acl" || fail "ACLs do not match: $acl"
[[ $(vm "sudo stat -c '%U' $path/hello.txt") == anna ]] || fail "the file does not belong to anna"

say "3. NFS, read-only to the VM itself, root squashed"
# Root is squashed; anna's UID is the same on this "client".
nfs=$(vm "sudo mkdir -p /mnt/nfstest && sudo mount -t nfs -o vers=4 127.0.0.1:$path /mnt/nfstest && sudo -u anna ls /mnt/nfstest; (sudo -u anna touch /mnt/nfstest/x 2>/dev/null && echo WRITABLE || true); (sudo ls /mnt/nfstest >/dev/null 2>&1 && echo ROOT-READS || true); sudo umount /mnt/nfstest")
echo "$nfs" | sed 's/^/   /'
grep -q hello.txt <<<"$nfs" || fail "NFS shows no files"
! grep -q WRITABLE <<<"$nfs" || fail "the read-only export is writable"
! grep -q ROOT-READS <<<"$nfs" || fail "root is not squashed"

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
[[ $(call GET /files/shares | jq length) == 0 ]] || fail "the administrator sees shares without a rule"
[[ $(status_as admin GET "$files/folder") == 404 ]] || fail "the administrator can list the share"

echo 'written in the browser' >"$work/web.txt"
code=$(status_as anna POST "$files/upload?path=album&name=from-web.txt" \
    -H 'Content-Type: application/octet-stream' --data-binary "@$work/web.txt")
[[ $code == 201 ]] || fail "anna's upload: $code $(cat "$work/body")"
[[ $(vm "sudo stat -c '%U' $path/album/from-web.txt") == anna ]] || fail "the upload does not belong to anna"
acl=$(vm "sudo getfacl -p $path/album/from-web.txt")
grep -q '^user:ben:r' <<<"$acl" || fail "the upload did not inherit the ACL: $acl"
smb_ok ben "$ben_pw" 'get album/from-web.txt from-web.txt'
vm "grep -q 'written in the browser' /tmp/from-web.txt" || fail "SMB shows ben something else"

listing=$(call_as ben GET "$files/folder")
jq -e '.entries | map(.name) | index("hello.txt") and index("album")' <<<"$listing" >/dev/null \
    || fail "ben's listing: $listing"
[[ $(jq .writable <<<"$listing") == false ]] || fail "ben's folder is writable: $listing"
[[ $(call_as ben GET "$files/download?path=hello.txt") == 'hello from anna' ]] || fail "ben's download"
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

job_succeeded "$(call_as anna POST "$files/transfer" '{"mode":"copy","paths":["album"],"target_share":'"$share_id"',"target_path":""}' | jq -r .id)"
vm "sudo test -f '$path/album (2)/from-web.txt'" || fail "the copy is missing"
[[ $(vm "sudo stat -c '%U' '$path/album (2)/inner.txt'") == anna ]] || fail "the copy does not belong to anna"
call_as anna POST "$files/rename" '{"path":"album (2)","name":"copy"}' | jq -e '.path == "copy"' >/dev/null || fail "rename"
job_succeeded "$(call_as anna POST "$files/delete" '{"paths":["copy"]}' | jq -r .id)"
vm "sudo test -f $path/.recycle/anna/copy/from-web.txt" || fail "the deleted folder is not in anna's recycle bin"
smb_ok anna "$anna_pw" 'ls album/from-web.txt'

say "5. ben loses access"
update=$(call PATCH "/shares/$share_id" "{\"access\":[{\"kind\":\"account\",\"id\":$anna,\"access\":\"read_write\"}]}")
job_succeeded "$(jq -er .job.id <<<"$update")"
# Recycle bins keep their own permissions; each is private anyway.
[[ $(vm "sudo find $path -path $path/.recycle/'*' -prune -o -print0 | sudo xargs -0 getfacl -p 2>/dev/null | grep -c 'user:ben' || true") == 0 ]] \
    || fail "ben's ACL entries remain"
smb_refused ben "$ben_pw" 'ls'
[[ $(status_as ben GET "$files/folder") == 404 ]] || fail "ben can still list the share"

say "6. Each writer's own recycle bin"
smb_ok anna "$anna_pw" 'del hello.txt'
vm "sudo test -f $path/.recycle/anna/hello.txt" || fail "the file is not in anna's recycle bin"
# A second writer: Samba purges files it cannot move to their bin.
update=$(call PATCH "/shares/$share_id" "{\"access\":[{\"kind\":\"account\",\"id\":$anna,\"access\":\"read_write\"},{\"kind\":\"account\",\"id\":$ben,\"access\":\"read_write\"}]}")
job_succeeded "$(jq -er .job.id <<<"$update")"
vm "echo 'from ben' > /tmp/ben.txt"
smb_ok ben "$ben_pw" 'put ben.txt; del ben.txt'
vm "sudo test -f $path/.recycle/ben/ben.txt" || fail "ben's file is not in his recycle bin"
smb_denied ben "$ben_pw" 'ls .recycle/anna/*'

say "7. Deleting the share and the pool"
job_succeeded "$(call POST "/shares/$share_id/delete" '{"confirm":"photos"}' | jq -r .id)"
vm "! findmnt -n $path" || fail "$path is still mounted"
job_succeeded "$(call POST "/storage/pools/$pool/destroy" "{\"confirm\":\"$pool\"}" | jq -r .id)"
for id in $anna $ben; do call DELETE "/accounts/$id" >/dev/null; done
say "Passed: SMB, NFS, file manager, ACLs, permission changes, recycle bins"

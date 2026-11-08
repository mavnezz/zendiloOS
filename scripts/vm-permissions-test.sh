#!/usr/bin/env bash
# Tests that the permissions on a share's files follow what changes
# elsewhere, end to end in the test VM, with the ACLs and owners as getfacl
# and stat read them from the disk (docs/architecture.md, 4.4):
#
#   1. a pool, the accounts anna and ben, the groups staff, interns, kids,
#      writers and readers
#   2. a share made with a denied group: ben, in staff (may write) and in
#      interns (denied), gets an entry that denies him on the new share's
#      folder at once, and can neither enter nor write it
#   3. a new member of a denied group: ben joins kids, denied on "photos",
#      while a walk through photos' many files is under way. The change is
#      answered at once, naming photos as following in the background; once
#      the walks have ended, the folder and the files deep down deny ben
#   4. a walk cut off: zendiloosd restarts while photos is walked; started
#      again, it walks photos on its own until the files have the rules
#   5. leaving a group that writes: ben, in writers and readers on "docs",
#      owns a file and a recycle bin he opened to anna; he leaves writers.
#      His file and his bin pass to root, the bin root's alone; ben still
#      reads, but no longer writes or changes the file's mode
#   6. a rollback of docs to a snapshot from before ben left brings his
#      file and bin back as they were; the files get the rules of now again
#   7. a disabled account: anna, who writes photos, is disabled; the
#      folder denies her and what she owns passes to root; enabled again,
#      she writes again
#   8. everything is deleted again
#
# Needs the VM running with the package installed and set up; account and
# password as for vm-storage-test.sh. Uses two free disks of the VM and
# leaves pools in use alone; restarts zendiloosd once.
set -euo pipefail

# The second VM unless told otherwise: the first holds a LAN test.
export ZENDILOOS_VM=${ZENDILOOS_VM:-2}

root=$(cd "$(dirname "$0")/.." && pwd)
base="https://127.0.0.1:${ZENDILOOS_HTTPS_PORT:-$((8442 + ${ZENDILOOS_VM:-1}))}"
api="$base/api/v1"
user=${ZENDILOOS_USER:-admin}
password=${ZENDILOOS_PASSWORD:-correct horse battery}
pool=permtest
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

# The permissions of a share's files being applied (walks), newest first.
walks_of() { call GET '/jobs?limit=200' | jq -c --arg share "$1" '[.[] | select(.kind == "sharing.apply_access" and .target == $share)]'; }
walking() { walks_of "$1" | jq -e 'any(.[]; .state == "running")' >/dev/null; }
walked_after() { walks_of "$1" | jq -e --argjson after "$2" 'any(.[]; .id > $after and .state == "succeeded")' >/dev/null; }
# Waits until no walk of the share runs any more; the last must have succeeded.
walks_done() {
    wait_for "the walks of $1" "! walking $1" 600
    local last
    last=$(walks_of "$1" | jq -r '.[0] | .state + " " + (.error_detail // "")')
    [[ $last == succeeded* ]] || fail "the last walk of $1: $last"
}

acl_of() { vm "sudo getfacl -cp '$1' 2>/dev/null"; }
# An entry as getfacl prints it, perhaps followed by what the mask leaves
# of it (`#effective:`).
has_entry() {
    local acl
    acl=$(acl_of "$1")
    grep -qE "^$2([[:space:]]|\$)" <<<"$acl" || fail "$1 lacks $2: $(tr '\n' ' ' <<<"$acl")"
}
lacks() {
    local acl
    acl=$(acl_of "$1")
    ! grep -q "^$2" <<<"$acl" || fail "$1 still has $2: $(tr '\n' ' ' <<<"$acl")"
}
owner_of() { vm "sudo stat -c '%U' '$1'"; }
as() { local who=$1; shift; vm "sudo -u $who sh -c \"$*\" 2>/dev/null"; }

rule() { jq -nc --arg kind "$1" --argjson id "$2" --arg access "$3" '{kind: $kind, id: $id, access: $access}'; }
rules() { jq -sc . <<<"$*"; }

say "Signing in as $user"
sign_in

say "Cleaning up an earlier run"
for name in photos docs projects; do
    for id in $(call GET /shares | jq -r --arg name "$name" '.[] | select(.name == $name) | .id'); do
        job_succeeded "$(call POST "/shares/$id/delete" "{\"confirm\":\"$name\"}" | jq -r .id)"
    done
done
for name in staff interns kids writers readers; do
    for id in $(call GET /groups | jq -r --arg name "$name" '.[] | select(.name == $name) | .id'); do
        call DELETE "/groups/$id" >/dev/null
    done
done
for name in anna ben; do
    for id in $(call GET /accounts | jq -r --arg name "$name" '.[] | select(.name == $name) | .id'); do
        call DELETE "/accounts/$id" >/dev/null
    done
done
if call GET /storage/pools | jq -e --arg pool "$pool" '.[] | select(.name == $pool)' >/dev/null; then
    job_succeeded "$(call POST "/storage/pools/$pool/destroy" "{\"confirm\":\"$pool\"}" | jq -r .id)"
fi

say "1. A pool, two accounts and five groups"
free=$(call GET /storage/disks | jq -c '[.[] | select((.usage == "unused" or .usage == "foreign") and .id_link != null and (.read_only | not)) | .name][:2]')
[[ $(jq length <<<"$free") == 2 ]] || fail "two free disks are needed, found $free"
plan=$(call POST /storage/pools/plan "{\"name\":\"$pool\",\"layout\":\"mirror\",\"disks\":$free}")
token=$(jq -er .token <<<"$plan") || fail "plan: $plan"
job_succeeded "$(call POST /storage/pools "{\"token\":\"$token\",\"confirm\":\"$pool\"}" | jq -r .id)"
anna=$(call POST /accounts '{"name":"anna","role":"user","password":"first secret phrase"}' | jq -er .id) || fail "anna"
ben=$(call POST /accounts '{"name":"ben","role":"user","password":"second secret phrase"}' | jq -er .id) || fail "ben"
group() { call POST /groups "{\"name\":\"$1\",\"members\":$2}" | jq -er .id || fail "group $1"; }
staff=$(group staff "[$ben]")
interns=$(group interns "[$ben]")
kids=$(group kids '[]')
writers=$(group writers "[$ben]")
readers=$(group readers "[$ben]")
share() { # name rules: the share's path, its id in $work/<name>.id
    local made
    made=$(call POST /shares "$(jq -nc --arg name "$1" --arg pool "$pool" --argjson access "$2" \
        '{name: $name, pool: $pool, access: $access}')")
    jq -er .id <<<"$made" >"$work/$1.id" || fail "share $1: $made"
    jq -er .path <<<"$made"
}

say "2. A new share denies the members of a denied group"
projects=$(share projects "$(rules "$(rule group "$staff" read_write)" "$(rule group "$interns" deny)")")
has_entry "$projects" 'user:ben:---'
has_entry "$projects" 'group:staff:rwx'
as ben "ls $projects" && fail "ben, an intern, lists projects"
as ben "touch $projects/ben.txt" && fail "ben, an intern, writes to projects"
echo "   ben is denied on $projects from the start"

say "3. A new member of a denied group, while a walk is under way"
photos=$(share photos "$(rules "$(rule account "$anna" read_write)" "$(rule account "$ben" read_write)" "$(rule group "$kids" deny)")")
photos_id=$(cat "$work/photos.id")
# Enough files that walking them takes a while; one deep down.
vm "sudo sh -c 'mkdir -p $photos/many/deep/a/b && cd $photos/many && seq -f f%g 1 60000 | xargs touch && echo x > $photos/many/deep/a/b/file.txt'"
has_entry "$photos/many/deep/a/b/file.txt" 'user:ben:rw-'
update=$(call PATCH "/shares/$photos_id" "{\"access\":$(rules "$(rule account "$anna" read_write)" "$(rule account "$ben" read_write)" "$(rule group "$kids" deny)" "$(rule group "$readers" read)")}")
first=$(jq -er .job.id <<<"$update") || fail "no walk: $update"
joined=$(call PATCH "/groups/$kids" "{\"members\":[$ben]}")
echo "   the walk before was $(call GET "/jobs/$first" | jq -r .state) when ben joined kids"
jq -e '.members == ['"$ben"']' <<<"$joined" >/dev/null || fail "ben did not join kids: $joined"
jq -e '.pending_shares | index("photos")' <<<"$joined" >/dev/null || fail "photos is not named as following: $joined"
walks_done photos
has_entry "$photos" 'user:ben:---'
has_entry "$photos/many/deep/a/b/file.txt" 'user:ben:---'
has_entry "$photos/many/f59999" 'user:ben:---'
as ben "cat $photos/many/deep/a/b/file.txt" && fail "ben, a kid now, reads the files"
echo "   ben is denied on the folder and deep down"

say "4. A walk cut off by a restart is made up"
update=$(call PATCH "/shares/$photos_id" "{\"access\":$(rules "$(rule account "$anna" read_write)" "$(rule account "$ben" read_write)" "$(rule group "$kids" deny)")}")
cut=$(jq -er .job.id <<<"$update") || fail "no walk: $update"
vm "sudo systemctl restart zendiloosd"
wait_for "zendiloosd" "curl -skf $api/health" 120
state=$(call GET "/jobs/$cut" | jq -r .state)
if [[ $state == interrupted ]]; then
    wait_for "photos walked again after the start" "walked_after photos $cut" 600
    echo "   walked again on its own after the restart"
else
    echo "   (the walk had ended before the restart: $state)"
fi
walks_done photos
lacks "$photos/many/deep/a/b/file.txt" 'group:readers'
has_entry "$photos/many/deep/a/b/file.txt" 'user:ben:---'

say "5. Leaving a group that writes hands over what one owns"
docs=$(share docs "$(rules "$(rule group "$writers" read_write)" "$(rule group "$readers" read)")")
docs_id=$(cat "$work/docs.id")
[[ $(vm "sudo stat -c '%U %a' $docs/.recycle/ben") == "ben 700" ]] || fail "ben, a writer, has no bin"
as ben "echo from ben > $docs/ben.txt" || fail "ben cannot write docs"
# ben opens his bin to everyone and to anna, as its owner may.
as ben "echo old > $docs/.recycle/ben/old.txt && chmod 0777 $docs/.recycle/ben && setfacl -m u:anna:rwx $docs/.recycle/ben" \
    || fail "ben cannot change his bin"
[[ $(owner_of "$docs/ben.txt") == ben ]] || fail "ben's file is not his"
label=$(call POST "/shares/$docs_id/snapshots" '{"note":"before ben left"}' | jq -er .label) || fail "no snapshot"
left=$(call PATCH "/groups/$writers" '{"members":[]}')
jq -e '.pending_shares == ["docs"]' <<<"$left" >/dev/null || fail "docs is not named as following: $left"
walks_done docs
handed_over() {
    [[ $(owner_of "$docs/ben.txt") == root ]] || fail "ben, a reader now, still owns his file"
    [[ $(vm "sudo stat -c '%U %a' $docs/.recycle/ben") == "root 700" ]] \
        || fail "ben's bin: $(vm "sudo stat -c '%U %a' $docs/.recycle/ben")"
    lacks "$docs/.recycle/ben" 'user:anna'
    [[ $(owner_of "$docs/.recycle/ben/old.txt") == root ]] || fail "what is in ben's bin is still his"
    as ben "cat $docs/ben.txt" >/dev/null || fail "ben cannot read his file any more"
    as ben "echo more >> $docs/ben.txt" && fail "ben, a reader, writes his own file"
    as ben "chmod 666 $docs/ben.txt" && fail "ben, a reader, changes his file's mode"
    return 0
}
handed_over
echo "   ben's file and bin are root's; he reads, and writes no more"

say "6. A rollback gives the files the rules of now again"
rolled=$(call POST "/shares/$docs_id/snapshots/$label/rollback" '{"confirm":"docs"}')
job_succeeded "$(jq -er .job.id <<<"$rolled")"
walks_done docs
handed_over
echo "   the snapshot's owners and bin did not come back"

say "7. A disabled account is denied on the files too"
as anna "echo from anna > $photos/anna.txt" || fail "anna cannot write photos"
disabled=$(call PATCH "/accounts/$anna" '{"disabled":true}')
jq -e '.disabled and (.pending_shares | index("photos"))' <<<"$disabled" >/dev/null || fail "disabling: $disabled"
walks_done photos
has_entry "$photos" 'user:anna:---'
[[ $(owner_of "$photos/anna.txt") == root ]] || fail "a disabled account still owns its file"
as anna "ls $photos" && fail "anna, disabled, lists photos"
enabled=$(call PATCH "/accounts/$anna" '{"disabled":false}')
jq -e '(.disabled | not) and (.pending_shares | index("photos"))' <<<"$enabled" >/dev/null || fail "enabling: $enabled"
walks_done photos
has_entry "$photos" 'user:anna:rwx'
as anna "echo more >> $photos/anna.txt" || fail "anna, enabled again, cannot write"

say "8. Cleaning up"
for name in photos docs projects; do
    job_succeeded "$(call POST "/shares/$(cat "$work/$name.id")/delete" "{\"confirm\":\"$name\"}" | jq -r .id)"
done
for id in $staff $interns $kids $writers $readers; do call DELETE "/groups/$id" >/dev/null; done
for id in $anna $ben; do call DELETE "/accounts/$id" >/dev/null; done
job_succeeded "$(call POST "/storage/pools/$pool/destroy" "{\"confirm\":\"$pool\"}" | jq -r .id)"

say "Passed: new shares, new members, cut-off walks, leaving writers, rollbacks and disabled accounts all reach the files"

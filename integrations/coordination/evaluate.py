"""Read-only intent/merge comparison. Reports candidates; performs no merge operations."""
import hashlib
import json
import json
from pathlib import Path
import re
import time

from paths import contains, git, identity, relative_path, repository
from registry import actors, fields, text, transaction


def entry(root, revision, path):
    raw = git(root, 'ls-tree', '-z', '--full-tree', revision, '--', path)
    if not raw:
        return None
    header = raw.split('\t', 1)[0].split()
    return {'mode': header[0], 'object': header[2]}


def claim_signature(rows):
    return sorted((row['id'], row['generation'], row['ended'], identity(row['worktree'])) for row in rows)


def check_claims(repo, expected):
    with transaction() as db:
        current = claim_signature(actors(db, repo, include_ended=True))
    if current != expected:
        raise ValueError('Ownership or task intent changed during review; inspect and review again')


def inspect(request):
    fields(request, ('worktree', 'main_ref'), ('worktrees', 'paths', 'owner_actor_ids'))
    owner_choices = request.get('owner_actor_ids', {})
    if not isinstance(owner_choices, dict) or len(owner_choices) > 32 or not all(isinstance(k, str) and isinstance(v, str) for k, v in owner_choices.items()):
        raise ValueError('owner_actor_ids must map selected worktree roots to inspected actor IDs')
    filters = request.get('paths', [])
    if not isinstance(filters, list) or len(filters) > 64:
        raise ValueError('paths must contain at most 64 exact files/directory prefixes')
    filters = [relative_path(path) for path in filters]
    deadline = time.monotonic() + 30
    report_bytes = 0
    anchor = repository(request['worktree'])
    root = anchor['worktree']
    main_ref = request['main_ref']
    if not isinstance(main_ref, str) or not main_ref.startswith(('refs/heads/', 'refs/remotes/')) or len(main_ref) > 512:
        raise ValueError('main_ref must be an explicit local ref such as refs/remotes/origin/main; fetch separately before inspecting')
    if git(root, 'check-ref-format', main_ref, allow_failure=True) is None:
        raise ValueError('main_ref must be a literal branch ref, not a revision expression')
    main = git(root, 'rev-parse', '--verify', '--end-of-options', main_ref + '^{commit}').strip()
    selected = request.get('worktrees')
    if selected is None:
        raw = git(root, 'worktree', 'list', '--porcelain', '-z')
        selected = [part[len('worktree '):] for part in raw.split('\0') if part.startswith('worktree ')]
    if not isinstance(selected, list) or not 1 <= len(selected) <= 32 or len(set(selected)) != len(selected):
        raise ValueError('Select 1..32 distinct worktree roots')
    with transaction() as db:
        declarations = actors(db, anchor['repo'], include_ended=True)
    registry_snapshot = claim_signature(declarations)
    trees = []
    unavailable = []
    seen_roots = set()
    for location in selected:
        if not Path(location).is_dir():
            unavailable.append({'worktree': location, 'reason': 'Worktree path is unavailable on this host; no files were removed or recreated'})
            continue
        info = repository(location)
        actual_root = identity(info['worktree'])
        if actual_root in seen_roots:
            raise ValueError('The same worktree was selected through multiple path aliases')
        seen_roots.add(actual_root)
        if info['repo'] != anchor['repo']:
            raise ValueError(f'Worktree belongs to another repository: {location}')
        bases = git(location, 'merge-base', '--all', main, info['head']).splitlines()
        if len(bases) != 1:
            raise ValueError('No unique merge base; resolve history explicitly')
        base = bases[0]
        dirty = git(location, 'status', '--porcelain=v1', '-z', '--untracked-files=all', '--ignore-submodules=all', '--', *filters)
        working = []
        tokens = iter(dirty.split('\0'))
        for token in tokens:
            if not token:
                continue
            item = {'status': token[:2], 'path': token[3:]}
            if 'R' in token[:2] or 'C' in token[:2]:
                item['from'] = next(tokens, '')
            working.append(item)
        if len(working) > 128:
            raise ValueError('More than 128 dirty/untracked paths; narrow request.paths')
        delta = git(location, 'diff', '--no-ext-diff', '--no-textconv', '--no-renames', '--name-status', '-z', base, info['head'], '--', *filters)
        parts = delta.split('\0')
        changes = []
        if len(parts) > 257:
            raise ValueError('More than 128 changed paths; narrow the worktree review')
        historical_owners = [row for row in declarations if identity(row['worktree']) == identity(info['worktree']) and row['declaration'] and row['declaration'].get('access') != 'read_only']
        active_owners = [row for row in historical_owners if not row['ended']]
        owners = active_owners or historical_owners
        selected_owner = next((v for k, v in owner_choices.items() if identity(k) == identity(info['worktree'])), None)
        if selected_owner:
            owners = [row for row in historical_owners if row['id'] == selected_owner]
            if len(owners) != 1:
                raise ValueError('Selected owner is not an applicable inspected declaration')
        integrated = git(location, 'merge-base', '--is-ancestor', info['head'], main, allow_failure=True) is not None
        for i in range(0, len(parts) - 1, 2):
            if time.monotonic() > deadline:
                raise ValueError('Inspection exceeded 30 seconds; select fewer worktrees or changes')
            kind, path = parts[i:i+2]
            base_entry = entry(location, base, path)
            proposed_entry = entry(location, info['head'], path)
            main_entry = entry(location, main, path)
            baseline = base_entry['object'] if base_entry else None
            proposed = proposed_entry['object'] if proposed_entry else None
            current = main_entry['object'] if main_entry else None
            in_scope = False
            if len(owners) == 1:
                in_scope = any(contains(owners[0]['worktree'], s['path'], path) for s in owners[0]['declaration']['scopes'])
            if integrated or main_entry == proposed_entry:
                category = 'already_integrated'
            elif not in_scope:
                category = 'outside_declared_scope'
            elif main_entry != base_entry:
                category = 'main_also_changed'
            else:
                category = 'candidate_for_review'
            sizes = []
            for oid in (baseline, proposed):
                if oid:
                    sizes.append(int(git(location, 'cat-file', '-s', oid.strip()).strip()))
            hunks = []
            patch_header = ''
            if max(sizes, default=0) <= 128_000:
                patch = git(location, 'diff', '--no-ext-diff', '--no-textconv', '--no-renames', '--no-color', '--unified=3', base, info['head'], '--', path)
                report_bytes += len(patch.encode())
                if report_bytes > 500_000:
                    raise ValueError('Patch evidence exceeds 500 KB; narrow the selected worktrees')
                pieces = re.split(r'(?m)(?=^@@ )', patch)
                patch_header = pieces[0]
                for hunk in pieces[1:]:
                    key = hashlib.sha256((path + '\0' + patch_header + hunk).encode()).hexdigest()
                    hunks.append({'id': key, 'unit_kind': 'text_hunk', 'header': hunk.splitlines()[0], 'preview': hunk[:600], 'patch': patch_header + hunk})
            if not hunks:
                # Preserve binary/large/empty/mode-only changes for explicit
                # review without sending their contents into model context.
                fingerprint = json.dumps([path, base_entry, main_entry, proposed_entry], sort_keys=True, separators=(',', ':'))
                hunks.append({'id': hashlib.sha256(fingerprint.encode()).hexdigest(),
                              'unit_kind': 'file_change', 'header': 'Whole-file or metadata change',
                              'preview': fingerprint[:600], 'patch': None})
            changes.append({'path': path, 'kind': kind, 'category': category, 'base_blob': baseline.strip() if baseline else None,
                            'main_blob': current.strip() if current else None, 'proposed_blob': proposed.strip() if proposed else None,
                            'base_mode': base_entry['mode'] if base_entry else None, 'main_mode': main_entry['mode'] if main_entry else None, 'proposed_mode': proposed_entry['mode'] if proposed_entry else None,
                            'hunks': hunks, 'isolation_supported': all(e is None or e['mode'] != '160000' for e in (base_entry, proposed_entry))})
        trees.append({'worktree': info['worktree'], 'head': info['head'], 'merge_base': base,
                      'dirty': bool(dirty), 'dirty_fingerprint': hashlib.sha256(dirty.encode()).hexdigest(),
                      'working_changes': working,
                      'owner': owners[0] if len(owners) == 1 else None, 'changes': changes})
        if repository(location)['head'] != info['head'] or git(location, 'status', '--porcelain=v1', '-z', '--untracked-files=all', '--ignore-submodules=all', '--', *filters) != dirty:
            raise ValueError('Worktree changed during inspection; inspect again')
    if git(root, 'rev-parse', '--verify', '--end-of-options', main_ref + '^{commit}').strip() != main:
        raise ValueError('Main ref moved during inspection; inspect again')
    check_claims(anchor['repo'], registry_snapshot)
    return {'schema_version': 1, 'repo': anchor['repo'], 'main_ref': main_ref, 'main_revision': main,
            'registry_snapshot': registry_snapshot,
            'worktrees': trees, 'selection_paths': filters or ['all'], 'unavailable_worktrees': unavailable, 'upstream_freshness': 'Local ref only; refresh the remote separately',
            'coverage': 'Committed branch deltas against merge base; dirty/untracked work preserved and blocks a merge plan. Semantic intent still requires review.'}


def evaluate(request):
    fields(request, ('snapshot_request', 'main_revision', 'reviews'))
    snapshot = inspect(request['snapshot_request'])
    if snapshot['main_revision'] != request['main_revision']:
        raise ValueError('Main changed since review; inspect and review again')
    reviews = request['reviews']
    if not isinstance(reviews, list) or len(reviews) > 4096:
        raise ValueError('reviews must be a bounded array of explicit hunk decisions')
    decisions = {}
    for review in reviews:
        fields(review, ('worktree', 'head', 'owner_actor_id', 'owner_generation', 'hunk_id', 'decision', 'reason'))
        text(review['owner_actor_id'], 'owner_actor_id')
        text(review['reason'], 'reason')
        if type(review['owner_generation']) is not int or review['owner_generation'] < 1:
            raise ValueError('owner_generation must be a positive integer')
        text(review['reason'], 'review reason')
        if review['decision'] not in ('include', 'exclude', 'defer') or not isinstance(review['owner_generation'], int) or isinstance(review['owner_generation'], bool) or review['owner_generation'] < 1:
            raise ValueError('Every hunk needs include/exclude/defer and a grounded reason')
        if not isinstance(review['hunk_id'], str) or not re.fullmatch('[0-9a-f]{64}', review['hunk_id']):
            raise ValueError('Review hunk_id must be the exact SHA-256 returned by inspect')
        key = (identity(review['worktree']), review['head'], review['hunk_id'])
        if key in decisions:
            raise ValueError('Duplicate hunk decision')
        decisions[key] = review
    seen = set()
    isolated = []
    isolated_files = []
    unresolved = list(snapshot['unavailable_worktrees'])
    for tree in snapshot['worktrees']:
        if tree['dirty']:
            unresolved.append({'worktree': tree['worktree'], 'reason': 'Dirty/untracked selected paths are preserved and require review', 'working_changes': tree['working_changes']})
        for change in tree['changes']:
            if change['category'] == 'already_integrated':
                continue
            for hunk in change['hunks']:
                key = (identity(tree['worktree']), tree['head'], hunk['id'])
                review = decisions.get(key)
                if review is None:
                    unresolved.append({'hunk_id': hunk['id'], 'reason': 'No intent review decision'})
                    continue
                seen.add(key)
                if not tree['owner'] or review['owner_actor_id'] != tree['owner']['id'] or review['owner_generation'] != tree['owner']['generation']:
                    raise ValueError('Ownership intent changed since review; inspect and review again')
                if review['decision'] == 'include':
                    if tree['dirty'] or change['category'] != 'candidate_for_review' or not change['isolation_supported']:
                        unresolved.append({'hunk_id': hunk['id'], 'reason': 'Dirty worktree, scope gap, or changed main requires resolution'})
                    elif hunk['unit_kind'] == 'text_hunk':
                        isolated.append({'worktree': tree['worktree'], 'head': tree['head'], 'path': change['path'], 'hunk_id': hunk['id'], 'patch': hunk['patch'], 'reason': review['reason'], 'task': tree['owner']['declaration']['task'], 'change_summary': tree['owner']['declaration']['change_summary']})
                    else:
                        isolated_files.append({'worktree': tree['worktree'], 'head': tree['head'], 'path': change['path'],
                            'review_id': hunk['id'], 'expected_main_blob': change['main_blob'], 'expected_main_mode': change['main_mode'],
                            'proposed_blob': change['proposed_blob'], 'proposed_mode': change['proposed_mode'], 'reason': review['reason'],
                            'task': tree['owner']['declaration']['task'], 'change_summary': tree['owner']['declaration']['change_summary'],
                            'integration_only_git_args': ['restore', '--source=' + tree['head'], '--worktree', '--', change['path']]})
                elif review['decision'] == 'defer':
                    unresolved.append({'hunk_id': hunk['id'], 'reason': review['reason']})
    if set(decisions) - seen:
        raise ValueError('Review refers to stale, unknown, or already-integrated hunks; inspect again')
    selected_by_path = {}
    windows_paths = bool(re.match(r'^[a-z]:/', identity(request['snapshot_request']['worktree'])))
    for item in isolated + isolated_files:
        path_key = item['path'].casefold() if windows_paths else item['path']
        selected_by_path.setdefault(path_key, []).append(item)
    rejected = set()
    duplicates = set()
    for path, selections in selected_by_path.items():
        if len({identity(item['worktree']) for item in selections}) <= 1 and len({item['path'] for item in selections}) == 1:
            continue
        variants = {(item.get('patch'), item.get('proposed_blob'), item.get('proposed_mode')) for item in selections}
        if len(variants) == 1:
            selections[0]['equivalent_sources'] = [
                {'worktree': item['worktree'], 'head': item['head'], 'reason': item['reason']}
                for item in selections[1:]]
            duplicates.update(id(item) for item in selections[1:])
        else:
            rejected.update(id(item) for item in selections)
            unresolved.append({'path': path, 'reason': 'Included changes from multiple worktrees affect the same file; review their combined result before choosing an integration operation',
                               'sources': [{'worktree': item['worktree'], 'head': item['head']} for item in selections]})
    isolated = [item for item in isolated if id(item) not in rejected and id(item) not in duplicates]
    isolated_files = [item for item in isolated_files if id(item) not in rejected and id(item) not in duplicates]
    check_claims(snapshot['repo'], snapshot['registry_snapshot'])
    return {'main_revision': snapshot['main_revision'], 'status': 'review_plan' if (isolated or isolated_files) and not unresolved else 'needs_review',
            'isolated_hunks': isolated, 'isolated_files': isolated_files, 'unresolved': unresolved,
            'merge_action': 'none', 'next_action': 'Apply chosen hunks in a separate integration worktree, inspect dependencies and run repository validation before merging. Never replace main with a worktree snapshot.'}

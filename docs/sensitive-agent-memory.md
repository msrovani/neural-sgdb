# Sensitive-agent memory — host responsibilities and limits

This doc is for the host and connector, not for the core.

The `neural-sgdb` core is a memory substrate. It stores, scopes, links,
curates and recalls memory. It does **not** evaluate content, decide consent,
identify vulnerable subjects, or guarantee protection for third-party data.

If your product writes memories about other people, treats health/education/
parental/behavioral data, or is used with vulnerable users, the system is
**insufficient on its own**. You must add host-side policy before relying on it.

## What the core does

- keeps memory scoped and entity-linked
- supports recall by scope, entity, path, time, and mode
- supports explicit forget, traceability and curation
- keeps the write path ADD-only by design

## What the core does not do

- classify whether a memory is about a third party
- decide whether consent exists or was revoked
- block recall because the context is sensitive
- enforce retention, deletion-by-law or parental rules
- protect minors, families, patients, students, employees, or any other class by
  itself

## Where the system is insufficient by itself

Do not assume the core is enough for:

- memories about minors
- parental or guardianship context
- health-related memory
- education records or behavioral tracking of students
- anything that derives from another person’s behavior, state or identity
- contexts where consent can be withdrawn or time-boxed

In those cases, the host is responsible for:

- deciding whether the product can store the memory at all
- classifying the memory before it reaches the connector
- filtering, expiring or removing it according to your policy
- auditing what was written and when
- acting on consent revocation

## Minimum protective setup for a host

A responsible host should:

1. Define memory categories in the host, not in the core.
2. Tag sensitive memories before storing them.
3. Restrict recall to the right scope at the connector level.
4. Block cross-scope recall when the host is not authorized to mix contexts.
5. Log sensitive writes lightly so they can be audited and removed later.
6. Use `forget` and scope removal when consent is revoked or retention expires.

## A simple rule for product teams

If your agent might read a memory in a context where another person’s data matters,
assume the memory is sensitive until your host policy says otherwise.

Do not let the connector’s convenience become your compliance posture.

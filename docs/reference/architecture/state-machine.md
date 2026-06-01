# uu-tiktok — state machine

The state machine is the durable record of the pipeline's progress. It lives in a sqlite database (one row per watched-video to process) and arbitrates between concurrent orchestrator workers via row-level claim contention.

## Schema and lifecycle states

(TBD — populated in T05)

### Schema overview

(TBD — populated in T05)

### Lifecycle states

(TBD — populated in T05)

### State-transition diagram

(TBD — populated in T05 with ASCII diagram)

## Claim contention

(TBD — populated in T05)

## Stale-claim sweep

(TBD — populated in T05)

## Mutator contracts

(TBD — populated in T05)

## Schema-version policy

(TBD — populated in T05)

## Crash-recovery durability

(TBD — populated in T05)

## Failure classification

(TBD — populated in T05)

## ADRs governing this subsystem

(TBD — populated in T05)

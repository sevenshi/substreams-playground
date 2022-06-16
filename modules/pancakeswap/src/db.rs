use std::string::String;

use substreams::pb::substreams::{store_delta, StoreDelta, Clock};
use substreams::{log, proto, store};

use crate::pb::database::table_change::Operation;
use crate::pb::database::{DatabaseChanges, Field, TableChange};
use crate::pcs::{Burn, Event, Events, Mint, Swap};
use crate::{field, field_create_string, field_from_strings, pb, pcs, utils, Type};

const PANCAKE_FACTORY: &str = "ca143ce32fe78f1f7019d7d551a6402fc5350c73";

#[derive(Clone, Debug)]
enum Item {
    PairDelta(StoreDelta),
    PcsTokenDelta(StoreDelta),
    TotalDelta(StoreDelta),
    VolumeDelta(StoreDelta),
    ReserveDelta(StoreDelta),
    Event(Event),
}

pub fn process(
    block: &Clock,
    pair_deltas: store::Deltas,
    pcs_token_deltas: store::Deltas,
    total_deltas: store::Deltas,
    volumes_deltas: store::Deltas,
    reserves_deltas: store::Deltas,
    events: Events,
    tokens: store::StoreGet,
) -> DatabaseChanges {
    let items = join_sort_deltas(
        pair_deltas,
        pcs_token_deltas,
        total_deltas,
        volumes_deltas,
        reserves_deltas,
        events,
    );

    log::info!("about! to process db_out items: {} ", items.len());

    let mut database_changes: DatabaseChanges = DatabaseChanges {
        table_changes: vec![],
    };

    for item in items {
        match item {
            Item::PairDelta(delta) => {
                handle_pair_delta(delta, &block, &mut database_changes, &tokens)
            }
            Item::PcsTokenDelta(delta) => handle_token_delta(delta, &mut database_changes, block),
            Item::TotalDelta(delta) => handle_total_delta(delta, &mut database_changes, block),
            Item::VolumeDelta(delta) => handle_volume_delta(delta, &mut database_changes, block),
            Item::ReserveDelta(delta) => handle_reserves_delta(delta, &mut database_changes, block),
            Item::Event(event) => handle_events(event, &mut database_changes, block),
        }
    }

    return database_changes;
}

fn handle_pair_delta(
    delta: StoreDelta,
    block: &Clock,
    changes: &mut DatabaseChanges,
    tokens: &store::StoreGet,
) {
    if delta.operation != store_delta::Operation::Create as i32 {
        return;
    }

    let pair: pcs::Pair = proto::decode(&delta.new_value).unwrap();

    log::info!("about! to process db_out token0_address:{} token1_address:{}  ",pair.token0_address.as_str(),pair.token1_address.as_str());
    let token0 = utils::get_last_token(&tokens, &pair.token0_address);
    let token1 = utils::get_last_token(&tokens, &pair.token1_address);

    changes.table_changes.push(TableChange {
        table: "pair".to_string(),
        pk: pair.address.clone(),
        block_num: block.number,
        ordinal: delta.ordinal,
        operation: delta.operation,
        fields: vec![
            field!("id", pair.address.clone(), ""),
            field!("name", format!("{}-{}", token0.symbol, token1.symbol), ""),
            field!("token_0", pair.token0_address, ""),
            field!("token_1", pair.token1_address, ""),
            field!("block", block.number, ""),
            field!("timestamp", block.timestamp.as_ref().unwrap().seconds, ""),
        ],
    });
}

fn handle_token_delta(delta: StoreDelta, changes: &mut DatabaseChanges, block: &Clock) {
    if delta.operation != store_delta::Operation::Create as i32 {
        return;
    }

    let token: pb::tokens::Token = proto::decode(&delta.new_value).unwrap();

    changes.table_changes.push(TableChange {
        table: "token".to_string(),
        pk: token.address.clone(),
        block_num: block.number,
        ordinal: delta.ordinal,
        operation: delta.operation,
        fields: vec![
            field!("id", token.address, ""),
            field!("name", token.name, ""),
            field!("symbol", token.symbol, ""),
            field!("decimals", token.decimals, ""),
        ],
    });
}

fn handle_total_delta(delta: StoreDelta, changes: &mut DatabaseChanges, block: &Clock) {
    let parts: Vec<&str> = delta.key.split(":").collect();
    let prefix = parts[0];
    let mut operation = delta.operation;
    let (table, pk, fields) = match prefix {
        "pair" => {
            let pair_address = parts[1];
            let field_name = parts[2];

            let field = match field_name {
                "transaction_count" => field_from_strings!("total_transactions", delta),
                "swap_count" => return, // todo: what does here ? up the colum of pancake_factory.swap[] ?
                "mint_count" => return, // todo: what does here ? up the colum of pancake_factory.mint[] ?
                "burn_count" => return, // todo: what does here ? up the colum of pancake_factory.burn[] ?
                _ => return,
            };

            ("pair", pair_address, vec![field])
        }
        "token" => {
            let token_addr = parts[1];
            let field_name = parts[2]; // will take in account token0 addr and token1 addr

            let field = match field_name {
                "transaction_count" => field_from_strings!("total_transactions", delta),
                _ => return,
            };

            ("token", token_addr, vec![field])
        }
        "global" => {
            let field_name = parts[1];

            let field = match field_name {
                "transaction_count" => {
                    operation = Operation::Update as i32;
                    field_from_strings!("total_transactions", delta)
                }
                "pair_count" => field_from_strings!("total_pairs", delta),
                _ => return,
            };

            ("pancake_factory", PANCAKE_FACTORY, vec![field])
        }
        "global_day" => {
            if delta.operation == Operation::Delete as i32 {
                return;
            }

            let day = parts[1];
            let field_name = parts[2];
            operation = Operation::Update as i32;
            let field = match field_name {
                "transaction_count" => field_from_strings!("total_transactions", delta),
                _ => return,
            };

            ("pancake_day_data", day, vec![field])
        }
        _ => return,
    };

    changes.table_changes.push(TableChange {
        block_num: block.number,
        ordinal: delta.ordinal,
        operation,
        table: table.to_string(),
        pk: pk.to_string(),
        fields,
    })
}

fn handle_volume_delta(delta: StoreDelta, changes: &mut DatabaseChanges, block: &Clock) {
    let parts: Vec<&str> = delta.key.split(":").collect();
    let prefix = parts[0];

    let mut operation = delta.operation;
    let (table, pk, fields) = match prefix {
        "pair_day" => {
            if delta.operation == Operation::Delete as i32 {
                return;
            }

            let day = parts[1];
            let pair_address = parts[2];
            let key = parts[3];

            let field = match key {
                "usd" => field_from_strings!("daily_volume_usd", delta),
                "token0" => field_from_strings!("daily_volume_token_0", delta),
                "token1" => field_from_strings!("daily_volume_token_1", delta),
                _ => return,
            };
            operation = Operation::Update as i32;
            (
                "pair_day_data",
                format!("{}-{}", pair_address, day),
                vec![field],
            )
        }
        "pair_hour" => {
            if delta.operation == Operation::Delete as i32 {
                return;
            }

            let hour = parts[1];
            let pair_address = parts[2];
            let key = parts[3];

            let field = match key {
                "usd" => field_from_strings!("hourly_volume_usd", delta),
                "token0" => field_from_strings!("hourly_volume_token_0", delta),
                "token1" => field_from_strings!("hourly_volume_token_1", delta),
                _ => return,
            };
            operation = Operation::Update as i32;
            (
                "pair_hour_data",
                format!("{}-{}", pair_address, hour),
                vec![field],
            )
        }
        "pair" => {
            let pair_address = parts[1];
            let field_name = parts[2];

            let field = match field_name {
                "usd" => field_from_strings!("volume_usd", delta),

                "token0" => field_from_strings!("volume_token0", delta),
                "token1" => field_from_strings!("volume_token1", delta),
                "total_supply" => field_from_strings!("total_supply", delta),
                _ => return,
            };

            ("pair", pair_address.to_string(), vec![field])
        }
        "token_day" => {
            if delta.operation == Operation::Delete as i32 {
                return;
            }

            let day = parts[1];
            let token_address = parts[2];
            let key = parts[3];

            let field = match key {
                "usd" => field_from_strings!("daily_volume_usd", delta),
                _ => return,
            };

            (
                "token_day_data",
                format!("{}-{}", token_address, day),
                vec![field],
            )
        }
        "token" => {
            let token_address = parts[1];
            let key = parts[2];

            let field = match key {
                "trade" => field_from_strings!("trade_volume", delta),
                "trade_usd" => field_from_strings!("trade_volume_usd", delta),
                "liquidity" => field_from_strings!("liquidity", delta),
                _ => return,
            };

            ("token", token_address.to_string(), vec![field])
        }
        "global" => {
            let key = parts[1];
            operation = Operation::Update as i32;
            let field = match key {
                "usd" => field_from_strings!("total_volume_usd", delta),
                "bnb" => field_from_strings!("total_volume_bnb", delta),
                "liquidity_usd" => field_from_strings!("total_liquidity_usd", delta),
                _ => return,
            };

            ("pancake_factory", PANCAKE_FACTORY.to_string(), vec![field])
        }
        "global_day" => {
            if delta.operation == Operation::Delete as i32 {
                return;
            }

            let day = parts[1];
            let key = parts[2];

            operation = Operation::Update as i32;
            let field = match key {
                "usd" => {
                    operation = delta.operation;
                    field_from_strings!("daily_volume_usd", delta)
                }
                "bnb" => field_from_strings!("daily_volume_bnb", delta),
                _ => return,
            };

            ("pancake_day_data", day.to_string(), vec![field])
        }
        _ => return,
    };

    changes.table_changes.push(TableChange {
        block_num: block.number,
        ordinal: delta.ordinal,
        operation,
        table: table.to_string(),
        pk: pk.to_string(),
        fields,
    })
}

fn handle_reserves_delta(delta: StoreDelta, changes: &mut DatabaseChanges, block: &Clock) {
    let parts: Vec<&str> = delta.key.split(":").collect();
    let prefix = parts[0];

    let mut operation = delta.operation;

    let (table, pk, fields) = match prefix {
        "pair_day" => {
            if delta.operation == Operation::Delete as i32 {
                return;
            }

            let day = parts[1];
            let pair_address = parts[2];
            let key = parts[3];

            let field = match key {
                "reserve0" => field_from_strings!("reserve_0", delta),
                "reserve1" => {
                    operation = Operation::Update as i32;
                    field_from_strings!("reserve_1", delta)
                }
                _ => return,
            };

            (
                "pair_day_data",
                format!("{}-{}", pair_address, day),
                vec![field],
            )
        }
        "pair_hour" => {
            if delta.operation == Operation::Delete as i32 {
                return;
            }

            let hour = parts[1];
            let pair_address = parts[2];
            let key = parts[3];

            let field = match key {
                "reserve0" => field_from_strings!("reserve_0", delta),
                "reserve1" => {
                    operation = Operation::Update as i32;
                    field_from_strings!("reserve_1", delta)
                }
                _ => return,
            };

            (
                "pair_hour_data",
                format!("{}-{}", pair_address, hour),
                vec![field],
            )
        }
        "price" => {
            let key = parts[3];

            let field = match key {
                "token0" => field_from_strings!("token_0_price", delta),
                "token1" => field_from_strings!("token_1_price", delta),
                _ => return,
            };

            let pair_address = parts[1];
            ("pair", pair_address.to_string(), vec![field])
        }
        "reserve" => {
            let key = parts[3];

            let field = match key {
                "reserve0" => field_from_strings!("reserve_0", delta),
                "reserve1" => field_from_strings!("reserve_1", delta),
                _ => return,
            };

            let pair_address = parts[1];
            ("pair", pair_address.to_string(), vec![field])
        }
        _ => return,
    };

    changes.table_changes.push(TableChange {
        table: table.to_string(),
        pk: pk.to_string(),
        block_num: block.number,
        ordinal: delta.ordinal,
        operation,
        fields,
    })
}

fn handle_events(event: Event, changes: &mut DatabaseChanges, block: &Clock) {
    match event.r#type.as_ref().unwrap() {
        Type::Swap(swap) => handle_swap_event(&swap, &event, changes, block),
        Type::Burn(burn) => handle_burn_event(&burn, &event, changes, block),
        Type::Mint(mint) => handle_mint_event(&mint, &event, changes, block),
    }
}

fn handle_swap_event(swap: &Swap, event: &Event, changes: &mut DatabaseChanges, block: &Clock) {
    changes.table_changes.push(TableChange {
        table: "swap".to_string(),
        pk: swap.id.to_string(),
        block_num: block.number,
        ordinal: event.log_ordinal,
        operation: Operation::Create as i32,
        fields: vec![
            field_create_string!("id", swap.id),
            field_create_string!("transaction", event.transaction_id),
            field_create_string!("timestamp", event.timestamp),
            field_create_string!("pair", event.pair_address),
            field_create_string!("token_0", event.token0),
            field_create_string!("token_1", event.token1),
            field_create_string!("sender", swap.sender),
            field_create_string!("from", swap.from),
            field_create_string!("amount_0_in", swap.amount0_in),
            field_create_string!("amount_1_in", swap.amount1_in),
            field_create_string!("amount_0_out", swap.amount0_out),
            field_create_string!("amount_1_out", swap.amount1_out),
            field_create_string!("to", swap.to),
            field_create_string!("amount_usd", swap.amount_usd),
            field_create_string!("log_index", event.log_ordinal),
        ],
    });
}

fn handle_burn_event(burn: &Burn, event: &Event, changes: &mut DatabaseChanges, block: &Clock) {
    changes.table_changes.push(TableChange {
        table: "burn".to_string(),
        pk: burn.id.to_string(),
        block_num: block.number,
        ordinal: event.log_ordinal,
        operation: Operation::Create as i32,
        fields: vec![
            field!("id", burn.id, ""),
            field!("transaction", event.transaction_id, ""),
            field!("pair", event.pair_address, ""),
            field!("token_0", event.token0, ""),
            field!("token_1", event.token1, ""),
            field!("liquidity", burn.liquidity, ""),
            field!("timestamp", event.timestamp, ""),
            field!("to", burn.to, ""),
            field!("sender", burn.sender, ""),
            field!("amount_0", burn.amount0, ""),
            field!("amount_1", burn.amount1, ""),
            field!("log_index", event.log_ordinal, ""),
            field!("amount_usd", burn.amount_usd, ""),
        ],
    });
}

fn handle_mint_event(mint: &Mint, event: &Event, changes: &mut DatabaseChanges, block: &Clock) {
    changes.table_changes.push(TableChange {
        table: "mint".to_string(),
        pk: mint.id.to_string(),
        block_num: block.number,
        ordinal: event.log_ordinal,
        operation: Operation::Create as i32,
        fields: vec![
            field!("id", mint.id, ""),
            field!("transaction", event.transaction_id, ""),
            field!("pair", event.pair_address, ""),
            field!("token_0", event.token0, ""),
            field!("token_1", event.token1, ""),
            field!("to", mint.to, ""),
            field!("liquidity", mint.liquidity, ""),
            field!("timestamp", event.timestamp, ""),
            field!("sender", mint.sender, ""),
            field!("amount_0", mint.amount0, ""),
            field!("amount_1", mint.amount1, ""),
            field!("log_index", event.log_ordinal, ""),
            field!("amount_usd", mint.amount_usd, ""),
        ],
    });
}

fn join_sort_deltas(
    pair_deltas: store::Deltas,
    pcs_token_deltas: store::Deltas,
    total_deltas: store::Deltas,
    volumes_deltas: store::Deltas,
    reserves_delta: store::Deltas,
    events: Events,
) -> Vec<Item> {
    struct SortableItem {
        ordinal: u64,
        item: Item,
    }

    let mut items: Vec<SortableItem> = Vec::new();

    for delta in pair_deltas {
        items.push(SortableItem {
            ordinal: delta.ordinal,
            item: Item::PairDelta(delta),
        })
    }

    for delta in pcs_token_deltas {
        items.push(SortableItem {
            ordinal: delta.ordinal,
            item: Item::PcsTokenDelta(delta),
        })
    }

    for delta in total_deltas {
        items.push(SortableItem {
            ordinal: delta.ordinal,
            item: Item::TotalDelta(delta),
        })
    }

    for delta in volumes_deltas {
        items.push(SortableItem {
            ordinal: delta.ordinal,
            item: Item::VolumeDelta(delta),
        })
    }

    for delta in reserves_delta {
        items.push(SortableItem {
            ordinal: delta.ordinal,
            item: Item::ReserveDelta(delta),
        })
    }

    for event in events.events {
        items.push(SortableItem {
            ordinal: event.log_ordinal,
            item: Item::Event(event),
        })
    }

    items.sort_by(|a, b| a.ordinal.cmp(&b.ordinal));
    return items.iter().map(|item| item.item.clone()).collect();
}

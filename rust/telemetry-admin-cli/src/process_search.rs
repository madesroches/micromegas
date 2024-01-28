use anyhow::{Context, Result};
use analytics::prelude::*;

pub async fn print_recent_processes(connection: &mut sqlx::AnyConnection) {
    for p in list_recent_processes(connection, None).await.unwrap() {
        let process_info = p.process_info.unwrap();
        println!(
            "{} {} {}",
            process_info.start_time, process_info.process_id, process_info.exe
        );
    }
}

pub async fn print_process_search(connection: &mut sqlx::AnyConnection, filter: &str) {
    for p in processes_by_name_substring(connection, filter)
        .await
        .with_context(|| "print_process_search")
        .unwrap()
    {
        println!("{} {} {}", p.start_time, p.process_id, p.exe);
    }
}

pub async fn print_process_tree(pool: &sqlx::AnyPool, root_process_id: &str) -> Result<()> {
    let mut connection = pool.acquire().await?;
    let root_process_info = find_process(&mut connection, root_process_id).await?;
    for_each_process_in_tree(pool, &root_process_info, 0, |process_info, rec_level| {
        println!(
            "{}{} {}",
            " ".repeat(rec_level as usize * 2),
            &process_info.process_id,
            &process_info.exe
        );
    })
    .await?;
    Ok(())
}

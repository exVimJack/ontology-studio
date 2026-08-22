//! 诊断：查 documents 表的 folder_path 实际值。
use memory::Memory;

fn main() {
    let db = std::path::PathBuf::from(r"C:\Users\think\AppData\Roaming\com.onto-studio.app\onto-studio.db");
    let mem = Memory::open(&db).unwrap();
    let folders = mem.list_folders().unwrap();
    println!("=== list_folders() ===");
    for f in &folders {
        println!("  {f:?}");
    }
    println!("=== list_folder_tree() ===");
    let tree = mem.list_folder_tree().unwrap();
    for n in &tree {
        println!("  {} (path={}) children={}", n.name, n.path, n.children.len());
    }
    println!("=== Inbox 下的文件 ===");
    let docs = mem.list_documents_by_folder(Some("/Inbox")).unwrap();
    println!("  count = {}", docs.len());
    for d in &docs {
        println!("  {:?}", d);
    }
}

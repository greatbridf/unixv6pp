#ifndef OPEN_FILE_MANAGER_H
#define OPEN_FILE_MANAGER_H

#include "INode.h"
#include "File.h"
#include "FileSystem.h"

/* Forward Declaration */
class OpenFileTable;
struct open_file_table;

File* f_alloc(struct open_file_table* oft);
void f_close(struct open_file_table* oft, File* file);

extern "C" Inode* InodeTable_get(short dev, int ino);
extern "C" void InodeTable_put(Inode*);
extern "C" bool InodeTable_is_loaded(short dev, int ino);
extern "C" void InodeTable_update();

#endif

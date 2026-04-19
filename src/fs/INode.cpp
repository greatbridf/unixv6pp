#include "INode.h"
#include "Kernel.h"

extern "C" void inode_read(Inode* pInode) {
	pInode->ReadI();
}

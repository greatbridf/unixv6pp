#include "INode.h"
#include "Kernel.h"

extern "C" Buf* compat_filesys_alloc(short dev) {
        return Kernel::Instance().GetFileSystem().Alloc(dev);
}

extern "C" bool compat_fs_readonly(short dev) {
        return Kernel::Instance().GetFileSystem().IsReadOnly(dev);
}

extern "C" void compat_filesys_free(short dev, int blkno) {
        return Kernel::Instance().GetFileSystem().Free(dev, blkno);
}

extern "C" void inode_read(Inode* pInode) {
	pInode->ReadI();
}

#include "OpenFileManager.h"
#include "Kernel.h"
#include "Video.h"

/*==============================class OpenFileTable===================================*/
extern "C" File* OpenFileTable_f_alloc(struct open_file_table*);
extern "C" void OpenFileTable_f_close(struct open_file_table*, File*);

File* f_alloc(struct open_file_table* oft) {
	User& u = Kernel::Instance().GetUser();
        File* retval = OpenFileTable_f_alloc(oft);

        if (!retval || User_get_error()) {
                Diagnose::Write("No Free File Struct\n");
                return NULL;
        }

        return retval;
}

void f_close(struct open_file_table* oft, File* file) {
        OpenFileTable_f_close(oft, file);
}

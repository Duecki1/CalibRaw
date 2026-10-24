#import "../../../crates/calibraw-ui/src/platform/ios_scene.m"

@interface CalibRawTestDismissal : NSObject
@property(nonatomic, copy) void (^completion)(void);
- (void)dismissViewControllerAnimated:(BOOL)animated completion:(void (^)(void))completion;
@end

@implementation CalibRawTestDismissal
- (void)dismissViewControllerAnimated:(BOOL)animated completion:(void (^)(void))completion {
    self.completion = completion;
}
@end

@interface CalibRawTestPhotoResult : NSObject
@property(nonatomic, strong) NSItemProvider *itemProvider;
@end
@implementation CalibRawTestPhotoResult
@end

static void record_result(void *context, const char *json) {
    NSMutableArray *responses = (__bridge NSMutableArray *)context;
    NSData *data = [[NSString stringWithUTF8String:json] dataUsingEncoding:NSUTF8StringEncoding];
    NSDictionary *response = [NSJSONSerialization JSONObjectWithData:data options:0 error:nil];
    NSCAssert(response, @"The picker must provide valid JSON");
    [responses addObject:response];
    NSString *staging = response[@"staging_directory"];
    if (staging) {
        [NSFileManager.defaultManager removeItemAtPath:staging error:nil];
    }
}

static CalibRawDocumentPickerDelegate *start_session(NSMutableArray *responses) {
    CalibRawDocumentPickerDelegate *delegate = [CalibRawDocumentPickerDelegate new];
    delegate.context = (__bridge void *)responses;
    delegate.callback = record_result;
    activeDocumentPicker = delegate;
    return delegate;
}

int main(void) {
    @autoreleasepool {
        NSMutableArray *responses = [NSMutableArray array];
        NSObject *chooser = [NSObject new];
        NSObject *photos = [NSObject new];
        CalibRawDocumentPickerDelegate *delegate = start_session(responses);
        delegate.activePresentation = (UIPresentationController *)photos;
        [delegate presentationControllerDidDismiss:(UIPresentationController *)chooser];
        NSCAssert(responses.count == 0, @"Dismissing the chooser must not cancel Photos");
        __weak CalibRawDocumentPickerDelegate *retainedDelegate = delegate;
        delegate = nil;
        NSCAssert(retainedDelegate, @"Photos must retain its callback session");

        CalibRawTestDismissal *dismissal = [CalibRawTestDismissal new];
        [retainedDelegate picker:(PHPickerViewController *)dismissal didFinishPicking:@[]];
        NSCAssert(responses.count == 0, @"Close must wait for dismissal completion");
        dismissal.completion();
        NSCAssert(responses.count == 1 && [responses[0][@"cancelled"] boolValue], @"Close must report cancellation");
        dismissal.completion();
        NSCAssert(responses.count == 1 && !activeDocumentPicker, @"Close must complete exactly once");
        dismissal.completion = nil;

        [responses removeAllObjects];
        delegate = start_session(responses);
        delegate.activePresentation = (UIPresentationController *)photos;
        [delegate presentationControllerDidDismiss:(UIPresentationController *)photos];
        [delegate presentationControllerDidDismiss:(UIPresentationController *)photos];
        NSCAssert(responses.count == 1 && !activeDocumentPicker, @"Interactive dismissal must complete exactly once");

        [responses removeAllObjects];
        delegate = start_session(responses);
        CalibRawTestPhotoResult *result = [CalibRawTestPhotoResult new];
        result.itemProvider = [NSItemProvider new];
        [delegate picker:(PHPickerViewController *)dismissal
            didFinishPicking:(NSArray<PHPickerResult *> *)@[result]];
        NSCAssert(responses.count == 0, @"Confirm must wait for dismissal completion");
        dismissal.completion();
        NSDate *deadline = [NSDate dateWithTimeIntervalSinceNow:2];
        while (!responses.count && deadline.timeIntervalSinceNow > 0) {
            [NSRunLoop.currentRunLoop runUntilDate:[NSDate dateWithTimeIntervalSinceNow:0.01]];
        }
        NSCAssert(responses.count == 1 && [responses[0][@"errors"] count] == 1,
            @"Confirm must deliver selected-provider results");
        NSCAssert(!activeDocumentPicker, @"Confirm must release the picker session");

        [responses removeAllObjects];
        delegate = start_session(responses);
        NSURL *inbox = [[NSURL fileURLWithPath:NSTemporaryDirectory() isDirectory:YES]
            URLByAppendingPathComponent:@"CalibRaw-test-Inbox" isDirectory:YES];
        [NSFileManager.defaultManager createDirectoryAtURL:inbox withIntermediateDirectories:YES
            attributes:nil error:nil];
        NSURL *pickedCopy = [inbox URLByAppendingPathComponent:@"photo.dng"];
        NSCAssert([[NSData data] writeToURL:pickedCopy atomically:NO], @"The test must create a picked copy");
        [delegate documentPicker:(UIDocumentPickerViewController *)dismissal didPickDocumentsAtURLs:@[pickedCopy]];
        NSCAssert(responses.count == 1 && [responses[0][@"paths"] count] == 1,
            @"Files must deliver the picked copy");
        NSString *staged = responses[0][@"paths"][0];
        NSCAssert([staged hasPrefix:responses[0][@"staging_directory"]]
            && [staged.lastPathComponent isEqualToString:@"photo.dng"],
            @"Files must move the picked copy into import staging under its own name");
        NSCAssert(![NSFileManager.defaultManager fileExistsAtPath:pickedCopy.path],
            @"Files must not leave the picked copy in its Inbox");
        [NSFileManager.defaultManager removeItemAtURL:inbox error:nil];
        puts("Picker lifecycle: chooser handoff, close, confirm, Files staging, and exactly-once completion passed");
    }
    return 0;
}
RSpec.describe Cloud do
  it "sets the url" do
    stub_request(:any, %r{example.com}).to_return(status: 200)
    expect(Cloud.new.url).to eq("https://example.com")
  end
end

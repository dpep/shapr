RSpec.describe "empty input" do
  it "has no size" do
    expect(Reader.new("").size).to eq(0)
  end
end

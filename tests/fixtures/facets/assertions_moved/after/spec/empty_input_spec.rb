RSpec.describe "empty input" do
  it "has no size" do
    expect(Reader.new("").size).to eq(0)
  end

  it "reads nothing from an empty input" do
    reader = Reader.new("")
    expect(reader.line).to be_nil
    expect(reader).to be_empty
  end
end

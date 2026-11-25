RSpec.describe Reader do
  it "reads a line" do
    expect(Reader.new("a\n").line).to eq("a")
  end

  it "reads nothing from an empty input" do
    reader = Reader.new("")
    expect(reader.line).to be_nil
    expect(reader).to be_empty
  end
end

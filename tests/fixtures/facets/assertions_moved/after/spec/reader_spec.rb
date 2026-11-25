RSpec.describe Reader do
  it "reads a line" do
    expect(Reader.new("a\n").line).to eq("a")
  end
end
